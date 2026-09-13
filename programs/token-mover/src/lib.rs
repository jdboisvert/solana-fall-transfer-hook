pub mod instructions;

use anchor_lang::prelude::*;

pub use instructions::*;

declare_id!("A5BaDDzEaLimZeeSp7yicPqnjiihwpk1WzmfZTswKshc");

#[program]
pub mod token_mover {
    use super::*;

    // Entry point for the main program
    pub fn transfer_with_hook<'info>(
        ctx: Context<'info, TransferWithHook<'info>>,
        amount: u64,
    ) -> Result<()> {
        transfer::handler(ctx, amount)
    }
}
