// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use super::new_test_context;
use aptos_api_test_context::{current_function_name, TestContext};
use aptos_cached_packages::aptos_stdlib;
use aptos_types::{
    account_address::AccountAddress,
    transaction::{EntryFunction, MultisigTransactionPayload},
};
use move_core_types::{
    ident_str,
    language_storage::{ModuleId, CORE_CODE_ADDRESS},
    value::{serialize_values, MoveValue},
};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_payload_succeeds() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            2,    /* 2-of-3 */
            1000, /* initial balance */
        )
        .await;
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account_1.address(), 1000);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload.clone())
        .await;
    // Owner 2 approves and owner 3 rejects. There are still 2 approvals total (owners 1 and 2) so
    // the transaction can still be executed.
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;
    context
        .reject_multisig_transaction(owner_account_3, multisig_account, 1)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // The multisig tx that transfers away 1000 APT should have succeeded.
    assert_eq!(0, context.get_apt_balance(multisig_account).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_existing_account() {
    let mut context = new_test_context(current_function_name!());
    let multisig_account = &mut context.create_account().await;
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    let owners = vec![
        owner_account_1.address(),
        owner_account_2.address(),
        owner_account_3.address(),
    ];
    context
        .create_multisig_account_with_existing_account(multisig_account, owners.clone(), 2, 1000)
        .await;
    assert_owners(&context, multisig_account.address(), owners).await;
    assert_signature_threshold(&context, multisig_account.address(), 2).await;

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account_1.address(), 1000);
    context
        .create_multisig_transaction(
            owner_account_1,
            multisig_account.address(),
            multisig_payload.clone(),
        )
        .await;
    // Owner 2 approves and owner 3 rejects. There are still 2 approvals total (owners 1 and 2) so
    // the transaction can still be executed.
    context
        .approve_multisig_transaction(owner_account_2, multisig_account.address(), 1)
        .await;
    context
        .reject_multisig_transaction(owner_account_3, multisig_account.address(), 1)
        .await;

    let org_multisig_balance = context.get_apt_balance(multisig_account.address()).await;
    let org_owner_1_balance = context.get_apt_balance(owner_account_1.address()).await;

    context
        .execute_multisig_transaction(owner_account_2, multisig_account.address(), 202)
        .await;

    // The multisig tx that transfers away 1000 APT should have succeeded.
    assert_eq!(
        org_multisig_balance - 1000,
        context.get_apt_balance(multisig_account.address()).await
    );
    assert_eq!(
        org_owner_1_balance + 1000,
        context.get_apt_balance(owner_account_1.address()).await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_to_update_owners() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    let owner_account_4 = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address()],
            2,
            0, /* initial balance */
        )
        .await;
    assert_eq!(0, context.get_apt_balance(multisig_account).await);

    // Add owners 3 and 4.
    let add_owners_payload = bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(CORE_CODE_ADDRESS, ident_str!("multisig_account").to_owned()),
            ident_str!("add_owners").to_owned(),
            vec![],
            serialize_values(&vec![MoveValue::vector_address(vec![
                owner_account_3.address(),
                owner_account_4.address(),
            ])]),
        ),
    ))
    .unwrap();
    context
        .create_multisig_transaction(
            owner_account_1,
            multisig_account,
            add_owners_payload.clone(),
        )
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // There should be 4 owners now.
    assert_owners(&context, multisig_account, vec![
        owner_account_1.address(),
        owner_account_2.address(),
        owner_account_3.address(),
        owner_account_4.address(),
    ])
    .await;

    let remove_owners_payload = bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(CORE_CODE_ADDRESS, ident_str!("multisig_account").to_owned()),
            ident_str!("remove_owners").to_owned(),
            vec![],
            serialize_values(&vec![MoveValue::vector_address(vec![
                owner_account_4.address()
            ])]),
        ),
    ))
    .unwrap();
    context
        .create_multisig_transaction(
            owner_account_1,
            multisig_account,
            remove_owners_payload.clone(),
        )
        .await;
    context
        .approve_multisig_transaction(owner_account_3, multisig_account, 2)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;
    // There should be 3 owners now that owner 4 has been kicked out.
    assert_owners(&context, multisig_account, vec![
        owner_account_1.address(),
        owner_account_2.address(),
        owner_account_3.address(),
    ])
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_update_signature_threshold() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address()],
            2,    /* 2-of-2 */
            1000, /* initial balance */
        )
        .await;

    // Change the signature threshold from 2-of-2 to 1-of-2
    let signature_threshold_payload = bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(CORE_CODE_ADDRESS, ident_str!("multisig_account").to_owned()),
            ident_str!("update_signatures_required").to_owned(),
            vec![],
            serialize_values(&vec![MoveValue::U64(1)]),
        ),
    ))
    .unwrap();
    context
        .create_multisig_transaction(
            owner_account_1,
            multisig_account,
            signature_threshold_payload.clone(),
        )
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // The signature threshold should be 1-of-2 now.
    assert_signature_threshold(&context, multisig_account, 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_insufficient_balance_to_cover_gas() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    // Owner 2 has no APT balance.
    let owner_account_2 = &mut context.gen_account();
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address()],
            1,
            1000, /* initial balance */
        )
        .await;

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account_1.address(), 1000);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload)
        .await;
    // Target transaction execution should fail because the owner 2 account has no balance for gas.
    context
        .execute_multisig_transaction(owner_account_2, multisig_account, 400)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_payload_and_failing_execution() {
    let mut context = new_test_context(current_function_name!());
    let owner_account = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(owner_account, vec![], 1, 1000)
        .await;
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);
    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account.address(), 2000);
    context
        .create_multisig_transaction(owner_account, multisig_account, multisig_payload.clone())
        .await;
    // Target transaction execution should fail because the multisig account only has 1000 APT but
    // is requested to send 2000.
    // The transaction should still succeed with the failure tracked on chain.
    context
        .execute_multisig_transaction(owner_account, multisig_account, 202)
        .await;

    // Balance didn't change since the target transaction failed.
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_payload_hash() {
    let mut context = new_test_context(current_function_name!());
    let owner_account = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(owner_account, vec![], 1, 1000)
        .await;
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);
    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account.address(), 1000);
    context
        .create_multisig_transaction_with_payload_hash(
            owner_account,
            multisig_account,
            multisig_payload.clone(),
        )
        .await;
    context
        .execute_multisig_transaction_with_payload(
            owner_account,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account.address().to_hex_literal(), "1000"],
            202,
        )
        .await;

    // The multisig tx that transfers away 1000 APT should have succeeded.
    assert_eq!(0, context.get_apt_balance(multisig_account).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_payload_hash_and_failing_execution() {
    let mut context = new_test_context(current_function_name!());
    let owner_account = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(owner_account, vec![], 1, 1000)
        .await;
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);
    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account.address(), 2000);
    context
        .create_multisig_transaction_with_payload_hash(
            owner_account,
            multisig_account,
            multisig_payload.clone(),
        )
        .await;

    // Target transaction execution should fail because the multisig account only has 1000 APT but
    // is requested to send 2000.
    // The transaction should still succeed with the failure tracked on chain.
    context
        .execute_multisig_transaction_with_payload(
            owner_account,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account.address().to_hex_literal(), "2000"],
            202,
        )
        .await;
    // Balance didn't change since the target transaction failed.
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_payload_not_matching_hash() {
    let mut context = new_test_context(current_function_name!());
    let owner_account = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(owner_account, vec![], 1, 1000)
        .await;
    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account.address(), 500);
    context
        .create_multisig_transaction_with_payload_hash(
            owner_account,
            multisig_account,
            multisig_payload,
        )
        .await;

    // The multisig transaction execution should fail due to the amount being different
    // (1000 vs 500).
    context
        .execute_multisig_transaction_with_payload(
            owner_account,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account.address().to_hex_literal(), "1000"],
            400,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_matching_payload() {
    let mut context = new_test_context(current_function_name!());
    let owner_account = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(owner_account, vec![], 1, 1000)
        .await;
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);
    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account.address(), 1000);
    context
        .create_multisig_transaction(owner_account, multisig_account, multisig_payload.clone())
        .await;
    context
        .execute_multisig_transaction_with_payload(
            owner_account,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account.address().to_hex_literal(), "1000"],
            202,
        )
        .await;

    // The multisig tx that transfers away 1000 APT should have succeeded.
    assert_eq!(0, context.get_apt_balance(multisig_account).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_with_mismatching_payload() {
    let mut context = new_test_context(current_function_name!());
    let owner_account = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(owner_account, vec![], 1, 1000)
        .await;
    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account.address(), 1000);
    context
        .create_multisig_transaction(owner_account, multisig_account, multisig_payload.clone())
        .await;

    // The multisig transaction execution should fail due to the payload mismatch
    // amount being different (1000 vs 2000).
    context
        .execute_multisig_transaction_with_payload(
            owner_account,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account.address().to_hex_literal(), "2000"],
            400,
        )
        .await;
    // Balance didn't change since the target transaction failed.
    assert_eq!(1000, context.get_apt_balance(multisig_account).await);

    // Excuting the transaction with the correct payload should succeed.
    context
        .execute_multisig_transaction_with_payload(
            owner_account,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account.address().to_hex_literal(), "1000"],
            202,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_simulation() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            1,    /* 1-of-3 */
            1000, /* initial balance */
        )
        .await;

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account_1.address(), 1000);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload.clone())
        .await;

    // Simulate the multisig tx
    let simulation_resp = context
        .simulate_multisig_transaction(
            owner_account_1,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account_1.address().to_hex_literal(), "1000"],
            200,
        )
        .await;
    // Validate that the simulation did successfully execute a transfer of 1000 coins from the
    // multisig account.
    let simulation_resp = &simulation_resp.as_array().unwrap()[0];
    assert!(simulation_resp["success"].as_bool().unwrap());
    let withdraw_event = &simulation_resp["events"].as_array().unwrap()[0];
    assert_eq!(
        withdraw_event["type"].as_str().unwrap(),
        "0x1::fungible_asset::Withdraw"
    );
    let withdrawn_amount = withdraw_event["data"]["amount"].as_str().unwrap();
    assert_eq!(withdrawn_amount, "1000");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_simulation_2_of_3() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            2,    /* 2-of-3 */
            1000, /* initial balance */
        )
        .await;

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account_1.address(), 1000);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload.clone())
        .await;

    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;

    // Simulate the multisig transaction
    let simulation_resp = context
        .simulate_multisig_transaction(
            owner_account_1,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account_1.address().to_hex_literal(), "1000"],
            200,
        )
        .await;
    // Validate that the simulation did successfully execute a transfer of 1000 coins from the
    // multisig account.
    let simulation_resp = &simulation_resp.as_array().unwrap()[0];
    assert!(simulation_resp["success"].as_bool().unwrap());
    let withdraw_event = &simulation_resp["events"].as_array().unwrap()[0];
    assert_eq!(
        withdraw_event["type"].as_str().unwrap(),
        "0x1::fungible_asset::Withdraw"
    );
    let withdrawn_amount = withdraw_event["data"]["amount"].as_str().unwrap();
    assert_eq!(withdrawn_amount, "1000");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_simulation_fail() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            1,    /* 1-of-3 */
            1000, /* initial balance */
        )
        .await;

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account_1.address(), 2000);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload.clone())
        .await;

    // Simulating transferring more than what the multisig account has should fail.
    let simulation_resp = context
        .simulate_multisig_transaction(
            owner_account_1,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account_1.address().to_hex_literal(), "2000"],
            200,
        )
        .await;
    let simulation_resp = &simulation_resp.as_array().unwrap()[0];
    let transaction_failed = &simulation_resp["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| {
            event["type"]
                .as_str()
                .unwrap()
                .contains("TransactionExecutionFailed")
        });
    assert!(transaction_failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_transaction_simulation_fail_2_of_3_insufficient_approvals() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            2,    /* 2-of-3 */
            1000, /* initial balance */
        )
        .await;

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account_1.address(), 2000);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload.clone())
        .await;

    // Simulating without sufficient approvals has should fail.
    let simulation_resp = context
        .simulate_multisig_transaction(
            owner_account_1,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account_1.address().to_hex_literal(), "1000"],
            200,
        )
        .await;
    let simulation_resp = &simulation_resp.as_array().unwrap()[0];
    assert!(!simulation_resp["success"].as_bool().unwrap());
    assert!(simulation_resp["vm_status"]
        .as_str()
        .unwrap()
        .contains("MULTISIG_TRANSACTION_INSUFFICIENT_APPROVALS"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_simulate_multisig_transaction_should_charge_gas_against_sender() {
    let mut context = new_test_context(current_function_name!());
    let owner_account = &mut context.create_account().await;
    let multisig_account = context
        .create_multisig_account(
            owner_account,
            vec![],
            1,  /* 1-of-1 */
            10, /* initial balance */
        )
        .await;
    assert_eq!(10, context.get_apt_balance(multisig_account).await);

    let multisig_payload = construct_multisig_txn_transfer_payload(owner_account.address(), 10);
    context
        .create_multisig_transaction(owner_account, multisig_account, multisig_payload.clone())
        .await;

    // This simulation should succeed because gas should be paid out of the sender account (owner),
    // not the multisig account itself.
    let simulation_resp = context
        .simulate_multisig_transaction(
            owner_account,
            multisig_account,
            "0x1::aptos_account::transfer",
            &[],
            &[&owner_account.address().to_hex_literal(), "10"],
            200,
        )
        .await;
    let simulation_resp = &simulation_resp.as_array().unwrap()[0];
    assert!(simulation_resp["success"].as_bool().unwrap());
}

async fn assert_owners(
    context: &TestContext,
    multisig_account: AccountAddress,
    mut expected_owners: Vec<AccountAddress>,
) {
    let multisig_account_resource = context
        .api_get_account_resource(
            multisig_account,
            "0x1",
            "multisig_account",
            "MultisigAccount",
        )
        .await;
    let mut owners = multisig_account_resource["data"]["owners"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .map(|address| AccountAddress::from_hex_literal(address.as_str().unwrap()).unwrap())
        .collect::<Vec<_>>();
    owners.sort();
    expected_owners.sort();
    assert_eq!(expected_owners, owners);
}

async fn assert_signature_threshold(
    context: &TestContext,
    multisig_account: AccountAddress,
    expected_signature_threshold: u64,
) {
    let multisig_account_resource = context
        .api_get_account_resource(
            multisig_account,
            "0x1",
            "multisig_account",
            "MultisigAccount",
        )
        .await;
    assert_eq!(
        expected_signature_threshold.to_string(),
        multisig_account_resource["data"]["num_signatures_required"]
            .as_str()
            .unwrap()
    );
}

fn construct_multisig_txn_transfer_payload(recipient: AccountAddress, amount: u64) -> Vec<u8> {
    bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(CORE_CODE_ADDRESS, ident_str!("aptos_account").to_owned()),
            ident_str!("transfer").to_owned(),
            vec![],
            serialize_values(&vec![MoveValue::Address(recipient), MoveValue::U64(amount)]),
        ),
    ))
    .unwrap()
}

fn construct_multisig_txn_publish_package_payload(
    metadata_serialized: Vec<u8>,
    code: Vec<Vec<u8>>,
) -> Vec<u8> {
    bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(CORE_CODE_ADDRESS, ident_str!("code").to_owned()),
            ident_str!("publish_package_txn").to_owned(),
            vec![],
            serialize_values(&vec![
                MoveValue::vector_u8(metadata_serialized),
                MoveValue::Vector(
                    code.into_iter()
                        .map(MoveValue::vector_u8)
                        .collect::<Vec<_>>(),
                ),
            ]),
        ),
    ))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_smart_contract_deployment() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    
    // Create a 2-of-3 multisig account
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            2,    /* 2-of-3 */
            10000, /* initial balance for gas */
        )
        .await;

    // Create a simple test module
    let module_source = format!(r#"
        module {}::test_module {{
            use std::signer;
            
            struct Counter has key {{
                value: u64,
            }}
            
            public entry fun initialize(account: &signer) {{
                move_to(account, Counter {{ value: 0 }});
            }}
            
            public entry fun increment(account: &signer) acquires Counter {{
                let counter = borrow_global_mut<Counter>(signer::address_of(account));
                counter.value = counter.value + 1;
            }}
            
            #[view]
            public fun get_count(addr: address): u64 acquires Counter {{
                borrow_global<Counter>(addr).value
            }}
        }}
    "#, multisig_account.to_hex_literal());

    // Build the package payload
    let package_payload = aptos_stdlib::publish_module_source("test_module", &module_source);
    let (metadata_serialized, code) = match package_payload {
        aptos_types::transaction::TransactionPayload::EntryFunction(entry_func) => {
            let args = entry_func.args();
            let metadata_serialized = bcs::from_bytes::<Vec<u8>>(args.get(0).unwrap()).unwrap();
            let code = bcs::from_bytes::<Vec<Vec<u8>>>(args.get(1).unwrap()).unwrap();
            (metadata_serialized, code)
        }
        _ => panic!("Expected EntryFunction payload"),
    };

    // Create multisig transaction for smart contract deployment
    let multisig_payload = construct_multisig_txn_publish_package_payload(metadata_serialized, code);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload.clone())
        .await;

    // Owner 2 approves the transaction
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;

    // Execute the multisig transaction to deploy the smart contract
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // Verify the module was deployed by checking if it exists
    let module_response = context
        .get(&format!(
            "/accounts/{}/module/{}",
            multisig_account.to_hex_literal(),
            "test_module"
        ))
        .await;
    
    // The module should exist and be accessible
    assert!(module_response["bytecode"].is_string());
    assert_eq!(module_response["abi"]["name"], "test_module");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_smart_contract_deployment_with_rejection() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    
    // Create a 3-of-3 multisig account (all must approve)
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            3,    /* 3-of-3 */
            10000, /* initial balance for gas */
        )
        .await;

    // Create a simple test module
    let module_source = format!(r#"
        module {}::rejected_module {{
            struct Data has key {{
                value: u64,
            }}
        }}
    "#, multisig_account.to_hex_literal());

    let package_payload = aptos_stdlib::publish_module_source("rejected_module", &module_source);
    let (metadata_serialized, code) = match package_payload {
        aptos_types::transaction::TransactionPayload::EntryFunction(entry_func) => {
            let args = entry_func.args();
            let metadata_serialized = bcs::from_bytes::<Vec<u8>>(args.get(0).unwrap()).unwrap();
            let code = bcs::from_bytes::<Vec<Vec<u8>>>(args.get(1).unwrap()).unwrap();
            (metadata_serialized, code)
        }
        _ => panic!("Expected EntryFunction payload"),
    };

    let multisig_payload = construct_multisig_txn_publish_package_payload(metadata_serialized, code);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload.clone())
        .await;

    // Owner 2 approves but owner 3 rejects
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;
    context
        .reject_multisig_transaction(owner_account_3, multisig_account, 1)
        .await;

    // Try to execute - should fail due to insufficient approvals (only 2 out of 3 required)
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 400)
        .await;

    // Verify the module was NOT deployed
    let module_response = context
        .expect_status_code(404)
        .get(&format!(
            "/accounts/{}/module/{}",
            multisig_account.to_hex_literal(),
            "rejected_module"
        ))
        .await;
    
    // Should get a 404 since the module doesn't exist
    assert!(module_response["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_smart_contract_deployment_and_execution() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    
    // Create a 2-of-2 multisig account
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address()],
            2,    /* 2-of-2 */
            10000, /* initial balance for gas */
        )
        .await;

    // Deploy a counter module
    let module_source = format!(r#"
        module {}::counter {{
            use std::signer;
            
            struct Counter has key {{
                value: u64,
            }}
            
            public entry fun initialize(account: &signer) {{
                move_to(account, Counter {{ value: 0 }});
            }}
            
            public entry fun increment(account: &signer) acquires Counter {{
                let counter = borrow_global_mut<Counter>(signer::address_of(account));
                counter.value = counter.value + 1;
            }}
            
            #[view]
            public fun get_count(addr: address): u64 acquires Counter {{
                borrow_global<Counter>(addr).value
            }}
        }}
    "#, multisig_account.to_hex_literal());

    let package_payload = aptos_stdlib::publish_module_source("counter", &module_source);
    let (metadata_serialized, code) = match package_payload {
        aptos_types::transaction::TransactionPayload::EntryFunction(entry_func) => {
            let args = entry_func.args();
            let metadata_serialized = bcs::from_bytes::<Vec<u8>>(args.get(0).unwrap()).unwrap();
            let code = bcs::from_bytes::<Vec<Vec<u8>>>(args.get(1).unwrap()).unwrap();
            (metadata_serialized, code)
        }
        _ => panic!("Expected EntryFunction payload"),
    };

    // Deploy the contract via multisig
    let deploy_payload = construct_multisig_txn_publish_package_payload(metadata_serialized, code);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, deploy_payload)
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // Now create a multisig transaction to initialize the counter
    let initialize_payload = bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(multisig_account, ident_str!("counter").to_owned()),
            ident_str!("initialize").to_owned(),
            vec![],
            serialize_values(&vec![]),
        ),
    ))
    .unwrap();

    context
        .create_multisig_transaction(owner_account_1, multisig_account, initialize_payload)
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 2)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // Create a multisig transaction to increment the counter
    let increment_payload = bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(multisig_account, ident_str!("counter").to_owned()),
            ident_str!("increment").to_owned(),
            vec![],
            serialize_values(&vec![]),
        ),
    ))
    .unwrap();

    context
        .create_multisig_transaction(owner_account_1, multisig_account, increment_payload)
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 3)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // Verify the counter was incremented by calling the view function
    let view_response = context
        .post(
            "/view",
            json!({
                "function": format!("{}::counter::get_count", multisig_account.to_hex_literal()),
                "type_arguments": [],
                "arguments": [multisig_account.to_hex_literal()]
            }),
        )
        .await;
    
    assert_eq!(view_response[0], "1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_smart_contract_deployment_insufficient_gas() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    
    // Create a multisig account with very low balance (insufficient for deployment)
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address()],
            2,    /* 2-of-2 */
            10,   /* very low balance */
        )
        .await;

    // Create a large module that will require more gas
    let module_source = format!(r#"
        module {}::large_module {{
            use std::signer;
            use std::vector;
            
            struct LargeData has key {{
                data: vector<u64>,
            }}
            
            public entry fun initialize(account: &signer) {{
                let data = vector::empty<u64>();
                let i = 0;
                while (i < 100) {{
                    vector::push_back(&mut data, i);
                    i = i + 1;
                }};
                move_to(account, LargeData {{ data }});
            }}
            
            public entry fun add_data(account: &signer, value: u64) acquires LargeData {{
                let large_data = borrow_global_mut<LargeData>(signer::address_of(account));
                vector::push_back(&mut large_data.data, value);
            }}
        }}
    "#, multisig_account.to_hex_literal());

    let package_payload = aptos_stdlib::publish_module_source("large_module", &module_source);
    let (metadata_serialized, code) = match package_payload {
        aptos_types::transaction::TransactionPayload::EntryFunction(entry_func) => {
            let args = entry_func.args();
            let metadata_serialized = bcs::from_bytes::<Vec<u8>>(args.get(0).unwrap()).unwrap();
            let code = bcs::from_bytes::<Vec<Vec<u8>>>(args.get(1).unwrap()).unwrap();
            (metadata_serialized, code)
        }
        _ => panic!("Expected EntryFunction payload"),
    };

    let multisig_payload = construct_multisig_txn_publish_package_payload(metadata_serialized, code);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, multisig_payload)
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;

    // Execution should fail due to insufficient balance to cover gas
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 400)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_multisig_smart_contract_upgrade() {
    let mut context = new_test_context(current_function_name!());
    let owner_account_1 = &mut context.create_account().await;
    let owner_account_2 = &mut context.create_account().await;
    let owner_account_3 = &mut context.create_account().await;
    
    // Create a 2-of-3 multisig account
    let multisig_account = context
        .create_multisig_account(
            owner_account_1,
            vec![owner_account_2.address(), owner_account_3.address()],
            2,    /* 2-of-3 */
            20000, /* higher balance for multiple deployments */
        )
        .await;

    // Deploy initial version of the module
    let module_source_v1 = format!(r#"
        module {}::upgradeable {{
            use std::signer;
            
            struct Data has key {{
                version: u64,
                value: u64,
            }}
            
            public entry fun initialize(account: &signer) {{
                move_to(account, Data {{ version: 1, value: 0 }});
            }}
            
            #[view]
            public fun get_version(addr: address): u64 acquires Data {{
                borrow_global<Data>(addr).version
            }}
        }}
    "#, multisig_account.to_hex_literal());

    let package_payload_v1 = aptos_stdlib::publish_module_source("upgradeable", &module_source_v1);
    let (metadata_serialized_v1, code_v1) = match package_payload_v1 {
        aptos_types::transaction::TransactionPayload::EntryFunction(entry_func) => {
            let args = entry_func.args();
            let metadata_serialized = bcs::from_bytes::<Vec<u8>>(args.get(0).unwrap()).unwrap();
            let code = bcs::from_bytes::<Vec<Vec<u8>>>(args.get(1).unwrap()).unwrap();
            (metadata_serialized, code)
        }
        _ => panic!("Expected EntryFunction payload"),
    };

    // Deploy v1 via multisig
    let deploy_payload_v1 = construct_multisig_txn_publish_package_payload(metadata_serialized_v1, code_v1);
    context
        .create_multisig_transaction(owner_account_1, multisig_account, deploy_payload_v1)
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 1)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // Initialize the module
    let initialize_payload = bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(multisig_account, ident_str!("upgradeable").to_owned()),
            ident_str!("initialize").to_owned(),
            vec![],
            serialize_values(&vec![]),
        ),
    ))
    .unwrap();

    context
        .create_multisig_transaction(owner_account_1, multisig_account, initialize_payload)
        .await;
    context
        .approve_multisig_transaction(owner_account_3, multisig_account, 2)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // Verify initial version
    let view_response_v1 = context
        .post(
            "/view",
            json!({
                "function": format!("{}::upgradeable::get_version", multisig_account.to_hex_literal()),
                "type_arguments": [],
                "arguments": [multisig_account.to_hex_literal()]
            }),
        )
        .await;
    
    assert_eq!(view_response_v1[0], "1");

    // Now upgrade to version 2
    let module_source_v2 = format!(r#"
        module {}::upgradeable {{
            use std::signer;
            
            struct Data has key {{
                version: u64,
                value: u64,
            }}
            
            public entry fun initialize(account: &signer) {{
                move_to(account, Data {{ version: 2, value: 0 }});
            }}
            
            public entry fun upgrade_version(account: &signer) acquires Data {{
                let data = borrow_global_mut<Data>(signer::address_of(account));
                data.version = 2;
            }}
            
            #[view]
            public fun get_version(addr: address): u64 acquires Data {{
                borrow_global<Data>(addr).version
            }}
        }}
    "#, multisig_account.to_hex_literal());

    let package_payload_v2 = aptos_stdlib::publish_module_source("upgradeable", &module_source_v2);
    let (metadata_serialized_v2, code_v2) = match package_payload_v2 {
        aptos_types::transaction::TransactionPayload::EntryFunction(entry_func) => {
            let args = entry_func.args();
            let metadata_serialized = bcs::from_bytes::<Vec<u8>>(args.get(0).unwrap()).unwrap();
            let code = bcs::from_bytes::<Vec<Vec<u8>>>(args.get(1).unwrap()).unwrap();
            (metadata_serialized, code)
        }
        _ => panic!("Expected EntryFunction payload"),
    };

    // Deploy v2 via multisig (this is an upgrade)
    let deploy_payload_v2 = construct_multisig_txn_publish_package_payload(metadata_serialized_v2, code_v2);
    context
        .create_multisig_transaction(owner_account_2, multisig_account, deploy_payload_v2)
        .await;
    context
        .approve_multisig_transaction(owner_account_3, multisig_account, 3)
        .await;
    context
        .execute_multisig_transaction(owner_account_2, multisig_account, 202)
        .await;

    // Call the upgrade function
    let upgrade_payload = bcs::to_bytes(&MultisigTransactionPayload::EntryFunction(
        EntryFunction::new(
            ModuleId::new(multisig_account, ident_str!("upgradeable").to_owned()),
            ident_str!("upgrade_version").to_owned(),
            vec![],
            serialize_values(&vec![]),
        ),
    ))
    .unwrap();

    context
        .create_multisig_transaction(owner_account_1, multisig_account, upgrade_payload)
        .await;
    context
        .approve_multisig_transaction(owner_account_2, multisig_account, 4)
        .await;
    context
        .execute_multisig_transaction(owner_account_1, multisig_account, 202)
        .await;

    // Verify the version was upgraded
    let view_response_v2 = context
        .post(
            "/view",
            json!({
                "function": format!("{}::upgradeable::get_version", multisig_account.to_hex_literal()),
                "type_arguments": [],
                "arguments": [multisig_account.to_hex_literal()]
            }),
        )
        .await;
    
    assert_eq!(view_response_v2[0], "2");
}
