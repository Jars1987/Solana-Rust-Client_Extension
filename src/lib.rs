use std::{collections::HashSet, sync::Arc};

use error::SolanaClientExtError;
use solana_account::AccountSharedData;
use solana_client::{rpc_client, rpc_config::RpcSimulateTransactionConfig};
use solana_clock::{Epoch, Slot};
use solana_compute_budget::compute_budget::{self, ComputeBudget};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use agave_feature_set::FeatureSet;
use solana_fee_structure::FeeStructure;
use solana_hash::Hash;
use solana_message::Message;
use solana_program_runtime::sysvar_cache;
use solana_pubkey::Pubkey;
use solana_rent::Rent;
use solana_signer::signers::Signers;
use solana_transaction_context::TransactionContext;
use solana_bpf_loader_program::syscalls::create_program_runtime_environment_v1;
// use solana_sdk::{
//     account::AccountSharedData,
//     compute_budget::ComputeBudgetInstruction,
//     message::Message,
//     signers::Signers,
//     transaction::{SanitizedTransaction, Transaction},
//     transaction_context::TransactionContext,
// };

use {
    solana_program_runtime::{
        invoke_context::{self, EnvironmentConfig, InvokeContext},
        loaded_programs::{ProgramCacheForTxBatch, ProgramRuntimeEnvironments},
    },
    solana_svm_transaction::svm_message::SVMMessage,
    solana_timings::{ExecuteDetailsTimings, ExecuteTimings},

};
use solana_svm::message_processor; // MessageProcessor::process_message;
use solana_transaction::{sanitized::SanitizedTransaction, Transaction};

mod error;

/// # RpcClientExt
///
/// `RpcClientExt` is an extension trait for the rust solana client.
/// This crate provides extensions for the Solana Rust client, focusing on compute unit estimation and optimization.
pub trait RpcClientExt {
    fn estimate_compute_units_unsigned_tx<'a, I: Signers + ?Sized>(
        &self,
        unsigned_transaction: &Transaction,
        signers: &'a I,
    ) -> Result<u64, Box<dyn std::error::Error + 'static>>;

    fn estimate_compute_units_msg<'a, I: Signers + ?Sized>(
        &self,
        msg: &Message,
        signers: &'a I,
    ) -> Result<u64, Box<dyn std::error::Error + 'static>>;

    fn optimize_compute_units_unsigned_tx<'a, I: Signers + ?Sized>(
        &self,
        unsigned_transaction: &mut Transaction,
        signers: &'a I,
    ) -> Result<u32, Box<dyn std::error::Error + 'static>>;

    fn optimize_compute_units_msg<'a, I: Signers + ?Sized>(
        &self,
        message: &mut Message,
        signers: &'a I,
    ) -> Result<u32, Box<dyn std::error::Error + 'static>>;
}

impl RpcClientExt for solana_client::rpc_client::RpcClient {
    fn estimate_compute_units_unsigned_tx<'a, I: Signers + ?Sized>(
        &self,
        transaction: &Transaction,
        _signers: &'a I,
    ) -> Result<u64, Box<dyn std::error::Error + 'static>> {
        // GET SVM MESSAGE
        let sanitized = SanitizedTransaction::try_from_legacy_transaction(
            Transaction::from(transaction.clone()),
            &HashSet::new(),
        );


        let compute_budget = ComputeBudget::default();
        let feature_set = FeatureSet::all_enabled();
        let fee_structure = FeeStructure::default();
        let lamports_per_signature = fee_structure.lamports_per_signature;

        //Get pubkeys from Tx
        let accounts = &transaction.message.account_keys;
        //call PRC client to get account shared data
        let mut accounts_data = vec![];
        for key in accounts {
            let data: AccountSharedData = self.get_account(&key).unwrap().into();
            accounts_data.push((*key, data));
        }

        // Get Invoke context
        let mut transaction_context = TransactionContext::new(accounts_data, Rent::default(), 0, 0);

        let runtime_env = Arc::new(
            create_program_runtime_environment_v1(&feature_set, &compute_budget, false, false)
                .unwrap(),
        );
        let sysvar_c = sysvar_cache::SysvarCache::default();

        let closure = |pubkey: &Pubkey| {
            // get epoch vote account stake
            0 // Return 0 if None
        };

        let env_config = EnvironmentConfig::new(
            Hash::default(),
            lamports_per_signature,
            300_000_000,
            &closure,
            Arc::new(feature_set.clone()),
            &sysvar_c,
        );

        //Get prog_cache
        let mut prog_cache = ProgramCacheForTxBatch::new(
            Slot::default(), //Slot
            
            //enviorements
            ProgramRuntimeEnvironments::default(),
            None,             //Option<ProgramRuntimeEnvironments>
            Epoch::default(), //Epoch
        );

        let mut invoke_context = InvokeContext::new(
            &mut transaction_context,             //&'a mut TransactionContext,,
            &mut prog_cache,                      //&'a mut ProgramCacheForTxBatch,
            env_config,                                  //EnvironmentConfig<'a>,
            None,                                 //Option<Rc<RefCell<LogCollector>>>,
            compute_budget.to_owned(),            //execution_cost: SVMTransactionExecutionCost,
            // SVMTransactionExecutionCost::Default, //SVMTransactionExecutionCost ??
        );

        // Get Timmings
        let mut timings = ExecuteTimings::default();

        //Get Used CUs
        let mut used_cu = 0u64;

        //Get your message processor

        let result_msg = message_processor::process_message(
            sanitized.unwrap().message(), //&impl SVMMessage
            &vec![],                       //&[Vec<IndexOfAccount>]
            &mut invoke_context,           //&mut InvokeContext,
            &mut timings,                  //&mut ExecuteTimings,
            &mut used_cu,                  // &mut u64,
        );

        Ok(used_cu)
    }

    fn estimate_compute_units_msg<'a, I: Signers + ?Sized>(
        &self,
        message: &Message,
        signers: &'a I,
    ) -> Result<u64, Box<dyn std::error::Error + 'static>> {
        let config = RpcSimulateTransactionConfig {
            sig_verify: true,
            ..RpcSimulateTransactionConfig::default()
        };
        let mut tx = Transaction::new_unsigned(message.clone());
        tx.sign(signers, self.get_latest_blockhash()?);
        let result = self.simulate_transaction_with_config(&tx, config)?;

        let consumed_cu = result.value.units_consumed.ok_or(Box::new(
            SolanaClientExtError::ComputeUnitsError(
                "Missing Compute Units from transaction simulation.".into(),
            ),
        ))?;

        if consumed_cu == 0 {
            return Err(Box::new(SolanaClientExtError::RpcError(
                "Transaction simulation failed.".into(),
            )));
        }

        Ok(consumed_cu)
    }

    fn optimize_compute_units_unsigned_tx<'a, I: Signers + ?Sized>(
        &self,
        transaction: &mut Transaction,
        signers: &'a I,
    ) -> Result<u32, Box<dyn std::error::Error + 'static>> {
        let optimal_cu =
            u32::try_from(self.estimate_compute_units_unsigned_tx(transaction, signers)?)?;
        let optimize_ix = ComputeBudgetInstruction::set_compute_unit_limit(
            optimal_cu.saturating_add(optimal_cu.saturating_div(100) * 20),
        );
        transaction
            .message
            .account_keys
            .push(solana_compute_budget_interface::id());
        let compiled_ix = transaction.message.compile_instruction(&optimize_ix);

        transaction.message.instructions.insert(0, compiled_ix);

        Ok(optimal_cu)
    }

    /// Simulates the transaction to get compute units used for the transaction
    /// and adds an instruction to the message to request
    /// only the required compute units from the ComputeBudget program
    /// to complete the transaction with this Message.
    ///
    /// ```
    /// use solana_client::rpc_client::RpcClient;
    /// use solana_client_ext::RpcClientExt;
    /// use solana_sdk::{
    ///     message::Message, signature::read_keypair_file, signer::Signer, system_instruction,
    ///     transaction::Transaction,
    /// };
    /// fn main() {
    ///     let rpc_client = RpcClient::new("https://api.devnet.solana.com");
    ///     let keypair = read_keypair_file("~/.config/solana/id.json").unwrap();
    ///     let keypair2 = read_keypair_file("~/.config/solana/_id.json").unwrap();
    ///     let created_ix = system_instruction::transfer(&keypair.pubkey(), &keypair2.pubkey(), 10000);
    ///     let mut msg = Message::new(&[created_ix], Some(&keypair.pubkey()));
    ///
    ///     let optimized_cu = rpc_client
    ///         .optimize_compute_units_msg(&mut msg, &[&keypair])
    ///         .unwrap();
    ///     println!("optimized cu {}", optimized_cu);
    ///
    ///     let tx = Transaction::new(&[keypair], msg, rpc_client.get_latest_blockhash().unwrap());
    ///     let result = rpc_client
    ///         .send_and_confirm_transaction_with_spinner(&tx)
    ///         .unwrap();
    ///
    ///     println!(
    ///         "sig https://explorer.solana.com/tx/{}?cluster=devnet",
    ///         result
    ///     );
    /// }
    ///
    ///
    /// ```
    fn optimize_compute_units_msg<'a, I: Signers + ?Sized>(
        &self,
        message: &mut Message,
        signers: &'a I,
    ) -> Result<u32, Box<dyn std::error::Error + 'static>> {
        let optimal_cu = u32::try_from(self.estimate_compute_units_msg(message, signers)?)?;
        let optimize_ix = ComputeBudgetInstruction::set_compute_unit_limit(
            optimal_cu.saturating_add(150 /*optimal_cu.saturating_div(100)*100*/),
        );
        message.account_keys.push(solana_compute_budget_interface::id());
        let compiled_ix = message.compile_instruction(&optimize_ix);
        message.instructions.insert(0, compiled_ix);

        Ok(optimal_cu)
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer, system_instruction};

    use super::*;

    #[test]
    fn cu() {
        let rpc_client = solana_client::rpc_client::RpcClient::new("https://api.devnet.solana.com");
        let new_keypair = Keypair::new();
        rpc_client
            .request_airdrop(&new_keypair.pubkey(), 50000)
            .unwrap();
        let transfer_ix =
            system_instruction::transfer(&new_keypair.pubkey(), &Pubkey::new_unique(), 10000);
        let mut msg = Message::new(&[transfer_ix], Some(&new_keypair.pubkey()));
        let _optimized_cu = rpc_client
            .optimize_compute_units_msg(&mut msg, &[&new_keypair])
            .unwrap();

        let blockhash = rpc_client.get_latest_blockhash().unwrap();
        let tx = Transaction::new(&[&new_keypair], msg, blockhash);
        let result = rpc_client
            .send_and_confirm_transaction_with_spinner(&tx)
            .unwrap();
        println!(
            "sig https://explorer.solana.com/tx/{}?cluster=devnet",
            result
        );
        println!("{:?}", tx);
    }
}
