use core::convert::Infallible;

use super::{
    cache::CacheState, plain_account::PlainStorage, BundleState, CacheAccount, TransitionState,
};
use crate::{db::EmptyDB, TransitionAccount};
use revm_interpreter::primitives::{
    db::{Database, DatabaseCommit},
    hash_map, Account, AccountInfo, Bytecode, HashMap, B160, B256, U256,
};

/// State of blockchain.
pub struct State<'a, DBError> {
    /// Cached state contains both changed from evm executiong and cached/loaded account/storages
    /// from database. This allows us to have only one layer of cache where we can fetch data.
    /// Additionaly we can introuduce some preloading of data from database.
    pub cache: CacheState,
    /// Optional database that we use to fetch data from. If database is not present, we will
    /// return not existing account and storage.
    pub database: Box<dyn Database<Error = DBError> + 'a>,
    /// Build reverts and state that gets applied to the state.
    pub transition_builder: Option<TransitionBuilder>,
    /// Is state clear enabled
    /// TODO: should we do it as block number, it would be easier.
    pub has_state_clear: bool,
    pub eval_times: EvalTimes,
}

/// TODO remove
#[derive(Debug, Clone, Default)]
pub struct EvalTimes {
    pub apply_evm_time: std::time::Duration,
    pub apply_transition_time: std::time::Duration,
    pub merge_time: std::time::Duration,
    pub get_code_time: std::time::Duration,
    pub get_account_time: std::time::Duration,
    pub get_storage_time: std::time::Duration,
}

#[derive(Debug, Clone, Default)]
pub struct TransitionBuilder {
    /// Block state, it aggregates transactions transitions into one state.
    pub transition_state: TransitionState,
    /// After block is finishes we merge those changes inside bundle.
    /// Bundle is used to update database and create changesets.
    pub bundle_state: BundleState,
}

impl TransitionBuilder {
    /// Take all transitions and merge them inside bundle state.
    /// This action will create final post state and all reverts so that
    /// we at any time revert state of bundle to the state before transition
    /// is applied.
    pub fn merge_transitions(&mut self) {
        let transition_state = self.transition_state.take();
        self.bundle_state
            .apply_block_substate_and_create_reverts(transition_state);
    }
}

/// For State that does not have database.
impl State<'_, Infallible> {
    pub fn new_with_cache(mut cache: CacheState, has_state_clear: bool) -> Self {
        cache.has_state_clear = has_state_clear;
        Self {
            cache,
            database: Box::new(EmptyDB::default()),
            transition_builder: None,
            has_state_clear,
            eval_times: Default::default(),
        }
    }

    pub fn new_cached_with_transition() -> Self {
        Self {
            cache: CacheState::default(),
            database: Box::new(EmptyDB::default()),
            transition_builder: Some(TransitionBuilder {
                transition_state: TransitionState::new(true),
                bundle_state: BundleState::default(),
            }),
            has_state_clear: true,
            eval_times: Default::default(),
        }
    }

    pub fn new() -> Self {
        Self {
            cache: CacheState::default(),
            database: Box::new(EmptyDB::default()),
            transition_builder: None,
            has_state_clear: true,
            eval_times: Default::default(),
        }
    }

    pub fn new_legacy() -> Self {
        Self {
            cache: CacheState::default(),
            database: Box::new(EmptyDB::default()),
            transition_builder: None,
            has_state_clear: false,
            eval_times: Default::default(),
        }
    }
}

impl<'a, DBError> State<'a, DBError> {
    /// Itereate over received balances and increment all account balances.
    /// If account is not found inside cache state it will be loaded from database.
    ///
    /// Update will create transitions for all accounts that are updated.
    pub fn increment_balances(
        &mut self,
        balances: impl IntoIterator<Item = (B160, u128)>,
    ) -> Result<(), DBError> {
        // make transition and update cache state
        let mut transitions = Vec::new();
        for (address, balance) in balances {
            let original_account = self.load_cache_account(address)?;
            transitions.push((address, original_account.increment_balance(balance)))
        }
        // append transition
        if let Some(transition_builder) = self.transition_builder.as_mut() {
            transition_builder
                .transition_state
                .add_transitions(transitions);
        }

        Ok(())
    }

    /// State clear EIP-161 is enabled in Spurious Dragon hardfork.
    pub fn enable_state_clear_eip(&mut self) {
        self.has_state_clear = true;
        self.cache.has_state_clear = true;
        self.transition_builder
            .as_mut()
            .map(|t| t.transition_state.set_state_clear());
    }

    pub fn new_with_transtion(db: Box<dyn Database<Error = DBError> + 'a>) -> Self {
        Self {
            cache: CacheState::default(),
            database: db,
            transition_builder: Some(TransitionBuilder {
                transition_state: TransitionState::new(true),
                bundle_state: BundleState::default(),
            }),
            has_state_clear: true,
            eval_times: Default::default(),
        }
    }

    pub fn insert_not_existing(&mut self, address: B160) {
        self.cache.insert_not_existing(address)
    }

    pub fn insert_account(&mut self, address: B160, info: AccountInfo) {
        self.cache.insert_account(address, info)
    }

    pub fn insert_account_with_storage(
        &mut self,
        address: B160,
        info: AccountInfo,
        storage: PlainStorage,
    ) {
        self.cache
            .insert_account_with_storage(address, info, storage)
    }

    /// Apply evm transitions to transition state.
    fn apply_transition(&mut self, transitions: Vec<(B160, TransitionAccount)>) {
        // add transition to transition state.
        if let Some(transition_builder) = self.transition_builder.as_mut() {
            // NOTE: can be done in parallel
            transition_builder
                .transition_state
                .add_transitions(transitions);
        }
    }

    /// Merge transitions to the bundle and crete reverts for it.
    pub fn merge_transitions(&mut self) {
        //let time = std::time::Instant::now();
        if let Some(builder) = self.transition_builder.as_mut() {
            builder.merge_transitions()
        }
        //self.eval_times.merge_time += time.elapsed();
    }

    pub fn load_cache_account(&mut self, address: B160) -> Result<&mut CacheAccount, DBError> {
        match self.cache.accounts.entry(address) {
            hash_map::Entry::Vacant(entry) => {
                let info = self.database.basic(address)?;
                let bundle_account = match info.clone() {
                    None => CacheAccount::new_loaded_not_existing(),
                    Some(acc) if acc.is_empty() => {
                        CacheAccount::new_loaded_empty_eip161(HashMap::new())
                    }
                    Some(acc) => CacheAccount::new_loaded(acc, HashMap::new()),
                };
                Ok(entry.insert(bundle_account))
            }
            hash_map::Entry::Occupied(entry) => Ok(entry.into_mut()),
        }
    }

    /// Takes changeset and reverts from state and replaces it with empty one.
    /// This will trop pending Transition and any transitions would be lost.
    ///
    /// TODO make cache aware of transitions dropping by having global transition counter.
    pub fn take_bundle(&mut self) -> BundleState {
        std::mem::replace(
            self.transition_builder.as_mut().unwrap(),
            TransitionBuilder {
                transition_state: TransitionState::new(self.has_state_clear),
                bundle_state: BundleState::default(),
            },
        )
        .bundle_state
    }
}

impl<'a, DBError> Database for State<'a, DBError> {
    type Error = DBError;

    fn basic(&mut self, address: B160) -> Result<Option<AccountInfo>, Self::Error> {
        //let time = std::time::Instant::now();
        let res = self.load_cache_account(address).map(|a| a.account_info());
        //self.eval_times.get_account_time += time.elapsed();
        res
    }

    fn code_by_hash(
        &mut self,
        code_hash: revm_interpreter::primitives::B256,
    ) -> Result<Bytecode, Self::Error> {
        //let time = std::time::Instant::now();
        let res = match self.cache.contracts.entry(code_hash) {
            hash_map::Entry::Occupied(entry) => Ok(entry.get().clone()),
            hash_map::Entry::Vacant(entry) => {
                let code = self.database.code_by_hash(code_hash)?;
                entry.insert(code.clone());
                Ok(code)
            }
        };
        //self.eval_times.get_code_time += time.elapsed();
        res
    }

    fn storage(&mut self, address: B160, index: U256) -> Result<U256, Self::Error> {
        //let time = std::time::Instant::now();
        // Account is guaranteed to be loaded.
        let res = if let Some(account) = self.cache.accounts.get_mut(&address) {
            // account will always be some, but if it is not, U256::ZERO will be returned.
            Ok(account
                .account
                .as_mut()
                .map(|account| match account.storage.entry(index) {
                    hash_map::Entry::Occupied(entry) => Ok(entry.get().clone()),
                    hash_map::Entry::Vacant(entry) => {
                        let value = self.database.storage(address, index)?;
                        entry.insert(value);
                        Ok(value)
                    }
                })
                .transpose()?
                .unwrap_or_default())
        } else {
            unreachable!("For accessing any storage account is guaranteed to be loaded beforehand")
        };
        //self.eval_times.get_storage_time += time.elapsed();
        res
    }

    fn block_hash(&mut self, number: U256) -> Result<B256, Self::Error> {
        // TODO maybe cache it.
        self.database.block_hash(number)
    }
}

impl<'a, DBError> DatabaseCommit for State<'a, DBError> {
    fn commit(&mut self, evm_state: HashMap<B160, Account>) {
        //let time = std::time::Instant::now();
        let transitions = self.cache.apply_evm_state(evm_state);
        //self.eval_times.apply_evm_time += time.elapsed();
        //let time = std::time::Instant::now();
        self.apply_transition(transitions);
        //self.eval_times.apply_transition_time += time.elapsed();
    }
}
