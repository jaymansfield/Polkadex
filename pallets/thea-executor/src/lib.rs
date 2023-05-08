#![cfg_attr(not(feature = "std"), no_std)]

/// Edit this file to define custom logic or remove it if it is not needed.
/// Learn more about FRAME and the core library of Substrate FRAME pallets:
/// <https://docs.substrate.io/reference/frame-pallets/>
pub use pallet::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mock;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{
		log,
		pallet_prelude::*,
		sp_runtime::SaturatedConversion,
		traits::{Currency, ExistenceRequirement, ReservableCurrency},
		PalletId,
	};
	use frame_system::pallet_prelude::*;
	use sp_runtime::{traits::AccountIdConversion, Saturating};
	use sp_std::vec::Vec;
	use thea_primitives::{Network, TheaIncomingExecutor, TheaOutgoingExecutor};

	use thea_primitives::types::{Deposit, Withdraw};

	#[pallet::pallet]
	#[pallet::generate_store(pub (super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	/// Configure the pallet by specifying the parameters and types on which it depends.
	#[pallet::config]
	pub trait Config: frame_system::Config + asset_handler::pallet::Config {
		/// Because this pallet emits events, it depends on the runtime's definition of an event.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		/// Balances Pallet
		type Currency: Currency<Self::AccountId> + ReservableCurrency<Self::AccountId>;
		/// Asset Create/ Update Origin
		type AssetCreateUpdateOrigin: EnsureOrigin<<Self as frame_system::Config>::RuntimeOrigin>;
		/// Something that executes the payload
		type Executor: thea_primitives::TheaOutgoingExecutor;
		/// Thea PalletId
		#[pallet::constant]
		type TheaPalletId: Get<PalletId>;
		/// Total Withdrawals
		#[pallet::constant]
		type WithdrawalSize: Get<u32>;
		/// Para Id
		type ParaId: Get<u32>;
	}

	#[pallet::storage]
	#[pallet::getter(fn pending_withdrawals)]
	pub(super) type PendingWithdrawals<T: Config> =
		StorageMap<_, Blake2_128Concat, Network, Vec<Withdraw>, ValueQuery>;

	/// Withdrawal Fees for each network
	#[pallet::storage]
	#[pallet::getter(fn witdrawal_fees)]
	pub(super) type WithdrawalFees<T: Config> =
		StorageMap<_, Blake2_128Concat, Network, u128, OptionQuery>;

	/// Withdrawal batches ready for signing
	#[pallet::storage]
	#[pallet::getter(fn ready_withdrawals)]
	pub(super) type ReadyWithdrawals<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		T::BlockNumber,
		Blake2_128Concat,
		Network,
		Vec<Withdraw>,
		ValueQuery,
	>;

	#[pallet::storage]
	#[pallet::getter(fn get_approved_deposits)]
	pub(super) type ApprovedDeposits<T: Config> =
		StorageMap<_, Blake2_128Concat, T::AccountId, Vec<Deposit<T::AccountId>>, ValueQuery>;

	// Pallets use events to inform users when important changes are made.
	// https://docs.substrate.io/main-docs/build/events-errors/
	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Deposit Approved event ( recipient, asset_id, amount))
		DepositApproved(u8, T::AccountId, u128, u128),
		/// Deposit claimed event ( recipient, number of deposits claimed )
		DepositClaimed(T::AccountId, u128, u128),
		/// Withdrawal Queued ( network, from, beneficiary, assetId, amount )
		WithdrawalQueued(Network, T::AccountId, Vec<u8>, u128, u128),
		/// Withdrawal Ready (Network id )
		WithdrawalReady(Network),
		/// Withdrawal Executed (network, Tx hash )
		WithdrawalExecuted(Network, sp_core::H256),
		// Thea Public Key Updated ( network, new session id )
		TheaKeyUpdated(Network, u32),
		/// Withdrawal Fee Set (NetworkId, Amount)
		WithdrawalFeeSet(u8, u128),
	}

	// Errors inform users that something went wrong.
	#[pallet::error]
	pub enum Error<T> {
		/// Error names should be descriptive.
		NoneValue,
		/// Errors should have helpful documentation associated with them.
		StorageOverflow,
		/// Failed To Decode
		FailedToDecode,
		/// Beneficiary Too Long
		BeneficiaryTooLong,
		/// Withdrawal Not Allowed
		WithdrawalNotAllowed,
		/// Withdrawal Fee Config Not Found
		WithdrawalFeeConfigNotFound,
		/// Asset Not Registered
		AssetNotRegistered,
		/// Amount cannot be Zero
		AmountCannotBeZero,
		/// Failed To Handle Parachain Deposit
		FailedToHandleParachainDeposit,
		/// Token Type Not Handled
		TokenTypeNotHandled,
		/// Bounded Vector Overflow
		BoundedVectorOverflow,
		/// Bounded vector not present
		BoundedVectorNotPresent,
		/// No Approved Deposit
		NoApprovedDeposit,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(block_no: T::BlockNumber) -> Weight {
			let pending_withdrawals = <ReadyWithdrawals<T>>::iter_prefix(
				block_no.saturating_sub(T::BlockNumber::from(1u8)),
			);
			for (network_id, withdrawal) in pending_withdrawals {
				// This is fine as this trait is not supposed to fail
				if T::Executor::execute_withdrawals(network_id, withdrawal.encode()).is_err() {
					log::error!("Error while executing withdrawals...");
				}
			}
			//TODO: Clean Storage
			Weight::default()
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// An example dispatchable that takes a singles value as a parameter, writes the value to
		/// storage and emits an event. This function must be dispatched by a signed extrinsic.
		#[pallet::call_index(0)]
		#[pallet::weight(Weight::default())]
		pub fn withdraw(
			origin: OriginFor<T>,
			asset_id: u128,
			amount: u128,
			beneficiary: Vec<u8>,
			pay_for_remaining: bool,
			network: Network,
		) -> DispatchResult {
			let user = ensure_signed(origin)?;
			// TODO: Check if beneficiary can decode to the correct type based on the given network.
			Self::do_withdraw(user, asset_id, amount, beneficiary, pay_for_remaining, network)?;
			Ok(())
		}

		/// Manually claim an approved deposit
		///
		/// # Parameters
		///
		/// * `origin`: User
		/// * `num_deposits`: Number of deposits to claim from available deposits,
		/// (it's used to parametrise the weight of this extrinsic)
		#[pallet::call_index(1)]
		#[pallet::weight(Weight::default())]
		pub fn claim_deposit(origin: OriginFor<T>, num_deposits: u32) -> DispatchResult {
			let user = ensure_signed(origin)?;

			let mut deposits = <ApprovedDeposits<T>>::get(&user);
			let length: u32 = deposits.len().saturated_into();
			let length: u32 = if length <= num_deposits { length } else { num_deposits };

			for _ in 0..length {
				if let Some(deposit) = deposits.pop() {
					if let Err(err) = Self::execute_deposit(deposit.clone(), &user) {
						deposits.push(deposit);
						// Save it back on failure
						<ApprovedDeposits<T>>::insert(&user, deposits.clone());
						return Err(err)
					}
				} else {
					break
				}
			}

			if !deposits.is_empty() {
				// If pending deposits are available, save it back
				<ApprovedDeposits<T>>::insert(&user, deposits)
			} else {
				<ApprovedDeposits<T>>::remove(&user);
			}

			Ok(())
		}

		/// Add Token Config
		///
		/// # Parameters
		///
		/// * `network_id`: Network Id.
		/// * `fee`: Withdrawal Fee.
		#[pallet::call_index(2)]
		#[pallet::weight(Weight::default())]
		pub fn set_withdrawal_fee(
			origin: OriginFor<T>,
			network_id: u8,
			fee: u128,
		) -> DispatchResult {
			ensure_root(origin)?;
			<WithdrawalFees<T>>::insert(network_id, fee);
			Self::deposit_event(Event::<T>::WithdrawalFeeSet(network_id, fee));
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		pub fn thea_account() -> T::AccountId {
			T::TheaPalletId::get().into_account_truncating()
		}

		pub fn do_withdraw(
			user: T::AccountId,
			asset_id: u128,
			amount: u128,
			beneficiary: Vec<u8>,
			pay_for_remaining: bool,
			network: Network,
		) -> Result<(), DispatchError> {
			ensure!(beneficiary.len() <= 1000, Error::<T>::BeneficiaryTooLong);

			let withdraw = Withdraw {
				asset_id,
				amount,
				destination: beneficiary.clone(),
				is_blocked: false,
				extra: Vec::new(),
			};
			let mut pending_withdrawals = <PendingWithdrawals<T>>::get(network);

			ensure!(
				pending_withdrawals.len() < T::WithdrawalSize::get() as usize,
				Error::<T>::WithdrawalNotAllowed
			);

			let mut total_fees =
				<WithdrawalFees<T>>::get(network).ok_or(Error::<T>::WithdrawalFeeConfigNotFound)?;

			if pay_for_remaining {
				// User is ready to pay for remaining pending withdrawal for quick withdrawal
				let extra_withdrawals_available =
					T::WithdrawalSize::get().saturating_sub(pending_withdrawals.len() as u32);
				total_fees = total_fees.saturating_add(
					total_fees.saturating_mul(extra_withdrawals_available.saturated_into()),
				)
			}

			// Pay the fees
			<T as Config>::Currency::transfer(
				&user,
				&Self::thea_account(),
				total_fees.saturated_into(),
				ExistenceRequirement::KeepAlive,
			)?;

			// Handle assets
			asset_handler::pallet::Pallet::<T>::handle_asset(asset_id, user.clone(), amount)?;

			pending_withdrawals.push(withdraw);

			Self::deposit_event(Event::<T>::WithdrawalQueued(
				network,
				user,
				beneficiary,
				asset_id,
				amount,
			));
			if (pending_withdrawals.len() >= T::WithdrawalSize::get() as usize) || pay_for_remaining
			{
				// If it is full then we move it to ready queue and update withdrawal nonce
				<ReadyWithdrawals<T>>::insert(
					<frame_system::Pallet<T>>::block_number(), //Block No
					network,
					pending_withdrawals.clone(),
				);
				Self::deposit_event(Event::<T>::WithdrawalReady(network));
				pending_withdrawals = Vec::default();
			}
			<PendingWithdrawals<T>>::insert(network, pending_withdrawals);
			Ok(())
		}

		pub fn do_deposit(payload: Vec<u8>) -> Result<(), DispatchError> {
			let deposits: Vec<Deposit<T::AccountId>> =
				Decode::decode(&mut &payload[..]).map_err(|_| Error::<T>::FailedToDecode)?;
			for deposit in deposits {
				<ApprovedDeposits<T>>::mutate(&deposit.recipient, |pending_deposits| {
					pending_deposits.push(deposit.clone())
				})
			}
			Ok(())
		}

		pub fn execute_deposit(
			deposit: Deposit<T::AccountId>,
			recipient: &T::AccountId,
		) -> Result<(), DispatchError> {
			asset_handler::pallet::Pallet::<T>::mint_thea_asset(
				deposit.asset_id,
				recipient.clone(),
				deposit.amount,
			)?;
			// Emit event
			Self::deposit_event(Event::<T>::DepositClaimed(
				recipient.clone(),
				deposit.asset_id,
				deposit.amount,
			));
			Ok(())
		}
	}

	impl<T: Config> TheaIncomingExecutor for Pallet<T> {
		fn execute_deposits(_: Network, deposits: Vec<u8>) {
			if let Err(error) = Self::do_deposit(deposits) {
				log::error!(target:"thea","Deposit Failed : {:?}", error);
			}
		}
	}
}
