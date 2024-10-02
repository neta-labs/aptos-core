// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use crate::{
    gas::get_gas_parameters,
    natives::aptos_natives_with_builder,
    prod_configs::{
        aptos_default_ty_builder, aptos_prod_ty_builder, aptos_prod_vm_config,
        get_timed_feature_override,
    },
};
use aptos_gas_algebra::DynamicExpression;
use aptos_gas_schedule::{AptosGasParameters, MiscGasParameters, NativeGasParameters};
use aptos_native_interface::SafeNativeBuilder;
use aptos_types::{
    chain_id::ChainId,
    on_chain_config::{
        ConfigurationResource, FeatureFlag, Features, OnChainConfig, TimedFeatures,
        TimedFeaturesBuilder,
    },
    state_store::StateView,
};
use aptos_vm_types::storage::StorageGasParameters;
use move_vm_runtime::{config::VMConfig, use_loader_v1_based_on_env, RuntimeEnvironment, WithRuntimeEnvironment, Module};
use std::sync::Arc;
use crossbeam::utils::CachePadded;
use once_cell::sync::Lazy;
use aptos_types::executable::ModulePath;
use aptos_types::state_store::state_key::StateKey;
use aptos_types::state_store::TStateView;
use aptos_types::vm::modules::{ModuleStorageEntry, ModuleStorageEntryInterface};
use move_binary_format::errors::{Location, PartialVMError, VMResult};
use move_core_types::account_address::AccountAddress;
use move_core_types::identifier::IdentStr;
use move_core_types::vm_status::StatusCode;
use move_vm_types::{module_cyclic_dependency_error, module_linker_error};
use parking_lot::RwLock;

/// A runtime environment which can be used for VM initialization and more. Contains features
/// used by execution, gas parameters, VM configs and global caches. Note that it is the user's
/// responsibility to make sure the environment is consistent, for now it should only be used per
/// block of transactions because all features or configs are updated only on per-block basis.
pub struct AptosEnvironment(Arc<Environment>);

impl AptosEnvironment {
    /// Returns new execution environment based on the current state.
    pub fn new(state_view: &impl StateView) -> Self {
        Self(Arc::new(Environment::new(state_view, false, None)))
    }

    /// Returns new execution environment based on the current state, also using the provided gas
    /// hook for native functions for gas calibration.
    pub fn new_with_gas_hook(
        state_view: &impl StateView,
        gas_hook: Arc<dyn Fn(DynamicExpression) + Send + Sync>,
    ) -> Self {
        Self(Arc::new(Environment::new(
            state_view,
            false,
            Some(gas_hook),
        )))
    }

    /// Returns new execution environment based on the current state, also injecting create signer
    /// native for government proposal simulation. Should not be used for regular execution.
    pub fn new_with_injected_create_signer_for_gov_sim(state_view: &impl StateView) -> Self {
        Self(Arc::new(Environment::new(state_view, true, None)))
    }

    /// Returns new environment but with delayed field optimization enabled. Should only be used by
    /// block executor where this optimization is needed. Note: whether the optimization will be
    /// enabled or not depends on the feature flag.
    pub fn new_with_delayed_field_optimization_enabled(state_view: &impl StateView) -> Self {
        let env = Environment::new(state_view, true, None).try_enable_delayed_field_optimization();
        Self(Arc::new(env))
    }

    /// Returns the [ChainId] used by this environment.
    #[inline]
    pub fn chain_id(&self) -> ChainId {
        self.0.chain_id
    }

    /// Returns the [Features] used by this environment.
    #[inline]
    pub fn features(&self) -> &Features {
        &self.0.features
    }

    /// Returns the [TimedFeatures] used by this environment.
    #[inline]
    pub fn timed_features(&self) -> &TimedFeatures {
        &self.0.timed_features
    }

    /// Returns the [VMConfig] used by this environment.
    #[inline]
    pub fn vm_config(&self) -> &VMConfig {
        self.0.runtime_environment.vm_config()
    }

    /// Returns the gas feature used by this environment.
    #[inline]
    pub fn gas_feature_version(&self) -> u64 {
        self.0.gas_feature_version
    }

    /// Returns the gas parameters used by this environment, and an error if they were not found
    /// on-chain.
    #[inline]
    pub fn gas_params(&self) -> &Result<AptosGasParameters, String> {
        &self.0.gas_params
    }

    /// Returns the storage gas parameters used by this environment, and an error if they were not
    /// found on-chain.
    #[inline]
    pub fn storage_gas_params(&self) -> &Result<StorageGasParameters, String> {
        &self.0.storage_gas_params
    }

    /// Returns true if create_signer native was injected for the government proposal simulation.
    /// Deprecated, and should not be used.
    #[inline]
    #[deprecated]
    pub fn inject_create_signer_for_gov_sim(&self) -> bool {
        #[allow(deprecated)]
        self.0.inject_create_signer_for_gov_sim
    }
}

impl Clone for AptosEnvironment {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl WithRuntimeEnvironment for AptosEnvironment {
    fn runtime_environment(&self) -> &RuntimeEnvironment {
        &self.0.runtime_environment
    }
}

struct Environment {
    /// Specifies the chain, i.e., testnet, mainnet, etc.
    chain_id: ChainId,

    /// Set of features enabled in this environment.
    features: Features,
    /// Set of timed features enabled in this environment.
    timed_features: TimedFeatures,

    /// Gas feature version used in this environment.
    gas_feature_version: u64,
    /// Gas parameters used in this environment. Error is stored if gas parameters were not found
    /// on-chain.
    gas_params: Result<AptosGasParameters, String>,
    /// Storage gas parameters used in this environment. Error is stored if gas parameters were not
    /// found on-chain.
    storage_gas_params: Result<StorageGasParameters, String>,

    runtime_environment: RuntimeEnvironment,

    /// True if we need to inject create signer native for government proposal simulation.
    /// Deprecated, and will be removed in the future.
    #[deprecated]
    inject_create_signer_for_gov_sim: bool,
}

impl Environment {
    fn new(
        state_view: &impl StateView,
        inject_create_signer_for_gov_sim: bool,
        gas_hook: Option<Arc<dyn Fn(DynamicExpression) + Send + Sync>>,
    ) -> Self {
        let mut features = Features::fetch_config(state_view).unwrap_or_default();

        // TODO(loader_v2): Remove before rolling out. This allows us to replay with V2.
        if use_loader_v1_based_on_env() {
            features.disable(FeatureFlag::ENABLE_LOADER_V2);
        } else {
            features.enable(FeatureFlag::ENABLE_LOADER_V2);
        }

        // If no chain ID is in storage, we assume we are in a testing environment.
        let chain_id = ChainId::fetch_config(state_view).unwrap_or_else(ChainId::test);
        let timestamp = ConfigurationResource::fetch_config(state_view)
            .map(|config| config.last_reconfiguration_time())
            .unwrap_or(0);

        let mut timed_features_builder = TimedFeaturesBuilder::new(chain_id, timestamp);
        if let Some(profile) = get_timed_feature_override() {
            timed_features_builder = timed_features_builder.with_override_profile(profile)
        }
        let timed_features = timed_features_builder.build();

        // TODO(Gas):
        //   Right now, we have to use some dummy values for gas parameters if they are not found
        //   on-chain. This only happens in a edge case that is probably related to write set
        //   transactions or genesis, which logically speaking, shouldn't be handled by the VM at
        //   all. We should clean up the logic here once we get that refactored.
        let (gas_params, storage_gas_params, gas_feature_version) =
            get_gas_parameters(&features, state_view);
        let (native_gas_params, misc_gas_params, ty_builder) = match &gas_params {
            Ok(gas_params) => {
                let ty_builder = aptos_prod_ty_builder(gas_feature_version, gas_params);
                (
                    gas_params.natives.clone(),
                    gas_params.vm.misc.clone(),
                    ty_builder,
                )
            },
            Err(_) => {
                let ty_builder = aptos_default_ty_builder();
                (
                    NativeGasParameters::zeros(),
                    MiscGasParameters::zeros(),
                    ty_builder,
                )
            },
        };

        let mut builder = SafeNativeBuilder::new(
            gas_feature_version,
            native_gas_params,
            misc_gas_params,
            timed_features.clone(),
            features.clone(),
            gas_hook,
        );
        let natives = aptos_natives_with_builder(&mut builder, inject_create_signer_for_gov_sim);
        let vm_config = aptos_prod_vm_config(&features, &timed_features, ty_builder);
        let runtime_environment = RuntimeEnvironment::new_with_config(natives, vm_config);

        #[allow(deprecated)]
        Self {
            chain_id,
            features,
            timed_features,
            gas_feature_version,
            gas_params,
            storage_gas_params,
            runtime_environment,
            inject_create_signer_for_gov_sim,
        }
    }

    fn try_enable_delayed_field_optimization(mut self) -> Self {
        if self.features.is_aggregator_v2_delayed_fields_enabled() {
            self.runtime_environment.enable_delayed_field_optimization();
        }
        self
    }
}

struct ModuleCache {
    invalidated: bool,
    modules: HashMap<StateKey, CachePadded<Arc<ModuleStorageEntry>>>,
}

impl ModuleCache {
    fn empty() -> Self {
        Self {
            invalidated: false,
            modules: HashMap::new(),
        }
    }

    fn flush(&mut self) {
        self.invalidated = false;
        self.modules.clear();
    }

    fn flush_if_invalidated_and_mark_valid(&mut self) {
        if self.invalidated {
            self.invalidated = false;
            self.modules.clear();
        }
    }

    pub fn traverse<K: ModulePath>(
        &mut self,
        entry: Arc<ModuleStorageEntry>,
        address: &AccountAddress,
        module_name: &IdentStr,
        base_view: &impl TStateView<Key = K>,
        visited: &mut HashSet<StateKey>,
        runtime_environment: &RuntimeEnvironment,
    ) -> VMResult<Arc<Module>> {
        let cm = entry.as_compiled_module();
        runtime_environment.paranoid_check_module_address_and_name(
            cm.as_ref(),
            address,
            module_name,
        )?;

        let size = entry.size_in_bytes();
        let hash = entry.hash();
        let locally_verified_module =
            runtime_environment.build_locally_verified_module(cm, size, hash)?;

        let mut verified_dependencies = vec![];
        for (addr, name) in locally_verified_module.immediate_dependencies_iter() {
            let dep_key = StateKey::from_address_and_module_name(addr, name);
            let dep_entry = match self.modules.get(&dep_key) {
                Some(dep_entry) => dep_entry.deref().clone(),
                None => {
                    let k = K::from_address_and_module_name(addr, name);
                    let sv = base_view
                        .get_state_value(&k)
                        .map_err(|_| {
                            PartialVMError::new(StatusCode::STORAGE_ERROR)
                                .finish(Location::Undefined)
                        })?
                        .ok_or_else(|| module_linker_error!(addr, name))?;
                    ModuleStorageEntry::from_state_value(runtime_environment, sv).map(Arc::new)?
                },
            };
            if let Some(module) = dep_entry.try_as_verified_module() {
                verified_dependencies.push(module);
                continue;
            }
            assert!(!dep_entry.is_verified());
            if visited.insert(dep_key.clone()) {
                let module = self.traverse(
                    dep_entry,
                    addr,
                    name,
                    base_view,
                    visited,
                    runtime_environment,
                )?;
                verified_dependencies.push(module);
            } else {
                return Err(module_cyclic_dependency_error!(address, module_name));
            }
        }

        // At this point, all dependencies of the module are verified, so we can run final checks
        // and construct a verified module.
        let module = Arc::new(
            runtime_environment
                .build_verified_module(locally_verified_module, &verified_dependencies)?,
        );
        let verified_entry = Arc::new(entry.make_verified(module.clone()));
        self.modules.insert(
            StateKey::from_address_and_module_name(address, module_name),
            CachePadded::new(verified_entry),
        );
        Ok(module)
    }
}

pub struct CrossBlockModuleCache(RwLock<ModuleCache>);

impl CrossBlockModuleCache {
    pub fn get_from_cross_block_module_cache(
        state_key: &StateKey,
    ) -> Option<Arc<ModuleStorageEntry>> {
        MODULE_CACHE.get_module_storage_entry(state_key)
    }

    pub fn store_to_cross_block_module_cache(state_key: StateKey, entry: Arc<ModuleStorageEntry>) {
        MODULE_CACHE.store_module_storage_entry(state_key, entry)
    }

    pub fn traverse<K: ModulePath>(
        entry: Arc<ModuleStorageEntry>,
        address: &AccountAddress,
        module_name: &IdentStr,
        base_view: &impl TStateView<Key = K>,
        runtime_environment: &RuntimeEnvironment,
    ) -> VMResult<Arc<Module>> {
        let mut cache = MODULE_CACHE.0.write();
        let mut visited = HashSet::new();
        cache.traverse(
            entry,
            address,
            module_name,
            base_view,
            &mut visited,
            runtime_environment,
        )
    }

    pub fn is_invalidated() -> bool {
        let cache = MODULE_CACHE.0.read();
        cache.invalidated
    }

    pub fn mark_invalid() {
        let mut cache = MODULE_CACHE.0.write();
        cache.invalidated = true;
    }

    pub fn flush_cross_block_module_cache_if_invalidated() {
        MODULE_CACHE.0.write().flush_if_invalidated_and_mark_valid()
    }

    pub fn flush() {
        MODULE_CACHE.0.write().flush()
    }

    fn empty() -> Self {
        Self(RwLock::new(ModuleCache::empty()))
    }

    fn get_module_storage_entry(&self, state_key: &StateKey) -> Option<Arc<ModuleStorageEntry>> {
        self.0.read().modules.get(state_key).map(|m| m.deref().clone())
    }

    fn store_module_storage_entry(&self, state_key: StateKey, entry: Arc<ModuleStorageEntry>) {
        let mut modules = self.0.write();
        modules.modules.insert(state_key, CachePadded::new(entry));
    }
}

static MODULE_CACHE: Lazy<CrossBlockModuleCache> = Lazy::new(CrossBlockModuleCache::empty);


#[cfg(test)]
pub mod test {
    use super::*;
    use aptos_language_e2e_tests::data_store::FakeDataStore;

    #[test]
    fn test_new_environment() {
        // This creates an empty state.
        let state_view = FakeDataStore::default();
        let env = Environment::new(&state_view, false, None);

        // Check default values.
        assert_eq!(&env.features, &Features::default());
        assert_eq!(env.chain_id.id(), ChainId::test().id());
        assert!(
            !env.runtime_environment
                .vm_config()
                .delayed_field_optimization_enabled
        );

        let env = env.try_enable_delayed_field_optimization();
        assert!(
            env.runtime_environment
                .vm_config()
                .delayed_field_optimization_enabled
        );
    }
}
