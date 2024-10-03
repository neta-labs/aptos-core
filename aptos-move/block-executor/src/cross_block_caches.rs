// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;
use arc_swap::{ArcSwap, ArcSwapOption};
use arc_swap::access::Access;
use aptos_types::state_store::StateView;
use aptos_vm_environment::environment::AptosEnvironment;
use bytes::Bytes;
use claims::assert_ok;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use aptos_types::executable::ModulePath;
use aptos_types::state_store::state_key::StateKey;
use aptos_types::vm::modules::{ModuleStorageEntry, ModuleStorageEntryInterface};
use aptos_vm_types::module_and_script_storage::AsAptosCodeStorage;
use move_core_types::account_address::AccountAddress;
use move_core_types::ident_str;
use move_vm_runtime::{ModuleStorage, RuntimeEnvironment};

/// Represents a unique identifier for an [AptosEnvironment] instance based on the features, gas
/// feature version, and other configs.
#[derive(Hash, Eq, PartialEq)]
struct EnvironmentID {
    bytes: Bytes,
}

impl EnvironmentID {
    /// Create a new identifier for the given environment.
    fn new(env: &AptosEnvironment) -> Self {
        // These are sufficient to distinguish different environments.
        let chain_id = env.chain_id();
        let features = env.features();
        let timed_features = env.timed_features();
        let gas_feature_version = env.gas_feature_version();
        let vm_config = env.vm_config();
        let bytes = bcs::to_bytes(&(
            chain_id,
            features,
            timed_features,
            gas_feature_version,
            vm_config,
        ))
        .expect("Should be able to serialize all configs")
        .into();
        Self { bytes }
    }
}

/// A cached environment that can be persisted across blocks. Used by block executor only. Also
/// stores an identifier so that we can check when it changes.
pub struct CachedAptosEnvironment {
    id: EnvironmentID,
    env: AptosEnvironment,
}

impl CachedAptosEnvironment {
    /// Returns the cached environment if it exists and has the same configuration as if it was
    /// created based on the current state, or creates a new one and caches it. Should only be
    /// called at the block boundaries.
    pub fn fetch_with_delayed_field_optimization_enabled(
        state_view: &impl StateView,
    ) -> AptosEnvironment {
        // Create a new environment.
        let env = AptosEnvironment::new_with_delayed_field_optimization_enabled(state_view);
        let id = EnvironmentID::new(&env);

        // Lock the cache, and check if the environment is the same.
        let mut cross_block_environment = CROSS_BLOCK_ENVIRONMENT.lock();
        if let Some(cached_env) = cross_block_environment.as_ref() {
            if id == cached_env.id {
                return cached_env.env.clone();
            }
        }

        // It is not, so we have to reset it.
        *cross_block_environment = Some(CachedAptosEnvironment {
            id,
            env: env.clone(),
        });
        drop(cross_block_environment);

        env
    }
}

static CROSS_BLOCK_ENVIRONMENT: Lazy<Mutex<Option<CachedAptosEnvironment>>> =
    Lazy::new(|| Mutex::new(None));

static MODULE_CACHE: Lazy<ArcSwapOption<HashMap<StateKey, Arc<ModuleStorageEntry>>>> = Lazy::new(|| ArcSwapOption::default());

pub fn maybe_initialize_module_cache(state_view: &impl StateView, runtime_environment: &RuntimeEnvironment) {
    let cache = MODULE_CACHE.load();
    if let Some(cache) = cache.deref() {
        let state_key = StateKey::module(&AccountAddress::ONE, ident_str!("transaction_validation"));
        if cache.get(&state_key).is_some() {
            return;
        }
    }

    let mut framework = HashMap::new();
    let module_storage = state_view.as_aptos_code_storage(runtime_environment);

    let ordered_module_names = [
        // Move stdlib.
        ident_str!("vector"),
        ident_str!("signer"),
        ident_str!("error"),
        ident_str!("hash"),
        ident_str!("features"),
        ident_str!("bcs"),
        ident_str!("option"),
        ident_str!("string"),
        ident_str!("fixed_point32"),

        // Aptos stdlib.
        ident_str!("type_info"),
        ident_str!("ed25519"),
        ident_str!("from_bcs"),
        ident_str!("multi_ed25519"),
        ident_str!("table"),
        ident_str!("bls12381"),
        ident_str!("math64"),
        ident_str!("fixed_point64"),
        ident_str!("math128"),
        ident_str!("math_fixed64"),
        ident_str!("table_with_length"),
        ident_str!("copyable_any"),
        ident_str!("simple_map"),
        ident_str!("bn254_algebra"),
        ident_str!("crypto_algebra"),
        ident_str!("aptos_hash"),

        // Framework.
        ident_str!("guid"),
        ident_str!("system_addresses"),
        ident_str!("chain_id"),
        ident_str!("timestamp"),
        ident_str!("event"),
        ident_str!("create_signer"),
        ident_str!("account"),
        ident_str!("aggregator"),
        ident_str!("aggregator_factory"),
        ident_str!("optional_aggregator"),
        ident_str!("transaction_context"),
        ident_str!("randomness"),
        ident_str!("object"),
        ident_str!("aggregator_v2"),
        ident_str!("function_info"),
        ident_str!("fungible_asset"),
        ident_str!("dispatchable_fungible_asset"),
        ident_str!("primary_fungible_store"),
        ident_str!("coin"),
        ident_str!("aptos_coin"),
        ident_str!("aptos_account"),
        ident_str!("chain_status"),
        ident_str!("staking_config"),
        ident_str!("stake"),
        ident_str!("transaction_fee"),
        ident_str!("transaction_validation"),
        ident_str!("reconfiguration_state"),
        ident_str!("state_storage"),
        ident_str!("storage_gas"),
        ident_str!("reconfiguration"),
        ident_str!("config_buffer"),
        ident_str!("randomness_api_v0_config"),
        ident_str!("randomness_config"),
        ident_str!("randomness_config_seqnum"),
        ident_str!("keyless_account"),
        ident_str!("consensus_config"),
        ident_str!("execution_config"),
        ident_str!("validator_consensus_info"),
        ident_str!("dkg"),
        ident_str!("gas_schedule"),
        ident_str!("util"),
        ident_str!("gas_schedule"),
        ident_str!("jwk_consensus_config"),
        ident_str!("jwks"),
        ident_str!("reconfiguration_with_dkg"),
        ident_str!("block"),
        ident_str!("code"),
    ];
    for module_name in ordered_module_names {
        let state_key = StateKey::module(&AccountAddress::ONE, module_name);
        let state_value = assert_ok!(state_view.get_state_value(&state_key));
        let module = assert_ok!(module_storage.fetch_verified_module(&AccountAddress::ONE, module_name));
        if let (Some(state_value), Some(module)) = (state_value, module) {
            let entry = ModuleStorageEntry::from_state_value_and_verified_module(state_value, module);
            framework.insert(state_key, Arc::new(entry));
        }
    }

    MODULE_CACHE.swap(Some(Arc::new(framework)));
}

pub(crate) fn get_cached<K: ModulePath>(key: &K) -> Option<Arc<ModuleStorageEntry>> {
    let cache = MODULE_CACHE.load_full();
    cache.and_then(|m| m.get(key.as_state_key()).cloned())
}
