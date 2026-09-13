#[allow(dead_code)]
mod helpers;

use {
    anchor_lang::{
        solana_program::instruction::{AccountMeta, Instruction},
        AccountDeserialize, Id, InstructionData, ToAccountMetas,
    },
    anchor_spl::token_2022::Token2022,
    solana_fall_transfer_hook::RateLimit,
    solana_keypair::{Address, Keypair},
    solana_message::{Message, VersionedMessage},
    solana_pubkey::Pubkey,
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
};

use helpers::{create_ata, mint_tokens, setup, setup_mint_and_extra_metas};

fn rate_limit_address(mint: &Pubkey, owner: &Pubkey, hook_program_id: &Address) -> Pubkey {
    Pubkey::find_program_address(
        &[b"rate_limit", mint.as_ref(), owner.as_ref()],
        hook_program_id,
    )
    .0
}

// Transfer through token-mover, which CPIs into Token-2022, which CPIs into the hook.
fn build_token_mover_ix(
    source_ata: &Pubkey,
    dest_ata: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
    hook_program_id: &Address,
    amount: u64,
) -> Instruction {
    let mut ix = Instruction::new_with_bytes(
        token_mover::id(),
        &token_mover::instruction::TransferWithHook { amount }.data(),
        token_mover::accounts::TransferWithHook {
            owner: *owner,
            source_token: *source_ata,
            mint: *mint,
            destination_token: *dest_ata,
            token_program: Token2022::id(),
        }
        .to_account_metas(None),
    );

    let extra_account_meta_list =
        Pubkey::find_program_address(&[b"extra-account-metas", mint.as_ref()], hook_program_id).0;

    let rate_limit = rate_limit_address(mint, owner, hook_program_id);

    // Remaining accounts: hook program first, then what it needs
    ix.accounts
        .push(AccountMeta::new_readonly(*hook_program_id, false));
    ix.accounts
        .push(AccountMeta::new_readonly(extra_account_meta_list, false));
    ix.accounts.push(AccountMeta::new(rate_limit, false));

    ix
}

#[test]
fn test_token_mover_transfer() {
    // Send 100 through your program
    let (mut svm, payer, program_id) = setup();
    let mint = Keypair::new();

    setup_mint_and_extra_metas(&mut svm, &payer, &mint, &program_id);

    let recipient = Keypair::new();
    let source_ata = create_ata(&mut svm, &payer, &payer.pubkey(), &mint.pubkey());
    let dest_ata = create_ata(&mut svm, &payer, &recipient.pubkey(), &mint.pubkey());

    mint_tokens(&mut svm, &payer, &mint.pubkey(), &source_ata, 1_000_000);

    let ix = build_token_mover_ix(
        &source_ata,
        &dest_ata,
        &mint.pubkey(),
        &payer.pubkey(),
        &program_id,
        100,
    );
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&payer]).unwrap();
    let res = svm.send_transaction(tx);
    assert!(
        res.is_ok(),
        "Transfer through token-mover failed: {:?}",
        res.err()
    );

    // A transfer that skipped the hook would also succeed, so confirm the
    // hook actually recorded the amount on the owner's rate limit account.
    let rate_limit = rate_limit_address(&mint.pubkey(), &payer.pubkey(), &program_id);
    let account = svm
        .get_account(&rate_limit)
        .expect("rate limit account should exist");
    let state = RateLimit::try_deserialize(&mut account.data.as_slice()).unwrap();
    assert_eq!(
        state.amount_transferred, 100,
        "hook did not run inside the CPI"
    );
}

#[test]
fn test_token_mover_rate_limit_exceeded() {
    // Send 1,000,000, then 1 more
    let (mut svm, payer, program_id) = setup();
    let mint = Keypair::new();

    setup_mint_and_extra_metas(&mut svm, &payer, &mint, &program_id);

    let recipient = Keypair::new();
    let source_ata = create_ata(&mut svm, &payer, &payer.pubkey(), &mint.pubkey());
    let dest_ata = create_ata(&mut svm, &payer, &recipient.pubkey(), &mint.pubkey());

    // Mint more than the limit so the second transfer can only fail on the hook,
    // not on insufficient funds
    mint_tokens(&mut svm, &payer, &mint.pubkey(), &source_ata, 2_000_000);

    // First transfer: exactly at the limit - should succeed
    let ix1 = build_token_mover_ix(
        &source_ata,
        &dest_ata,
        &mint.pubkey(),
        &payer.pubkey(),
        &program_id,
        1_000_000,
    );
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix1], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&payer]).unwrap();
    let res = svm.send_transaction(tx);
    assert!(
        res.is_ok(),
        "Transfer at limit should succeed: {:?}",
        res.err()
    );

    // Second transfer: 1 more - the hook must reject it with RateLimitExceeded (0x1771)
    let ix2 = build_token_mover_ix(
        &source_ata,
        &dest_ata,
        &mint.pubkey(),
        &payer.pubkey(),
        &program_id,
        1,
    );
    let blockhash = svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[ix2], Some(&payer.pubkey()), &blockhash);
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[&payer]).unwrap();
    let res = svm.send_transaction(tx);

    let failed = res.expect_err("Transfer exceeding rate limit should fail");
    assert!(
        failed
            .meta
            .logs
            .iter()
            .any(|log| log.contains("custom program error: 0x1771")),
        "expected RateLimitExceeded (0x1771), got: {:?}",
        failed.meta.logs
    );
}
