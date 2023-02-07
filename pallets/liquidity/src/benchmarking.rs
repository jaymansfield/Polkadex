// This file is part of Polkadex.

// Copyright (C) 2020-2023 Polkadex oü.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

//! Benchmarking setup for liquidity pallet
#![cfg(feature = "runtime-benchmarks")]
use super::*;
use crate::{pallet::Call, Pallet as liquidity};
use frame_benchmarking::benchmarks;
use frame_support::{dispatch::UnfilteredDispatchable, traits::EnsureOrigin};
use frame_system::RawOrigin;
use parity_scale_codec::Decode;
use polkadex_primitives::{AssetId, UNIT_BALANCE};
use sp_runtime::SaturatedConversion;
use thea_primitives::liquidity::LiquidityModifier;

// Check if last event generated by pallet is the one we're expecting
fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
	frame_system::Pallet::<T>::assert_last_event(generic_event.into());
}

#[cfg(feature = "runtime-benchmarks")]
benchmarks! {
	register_account {
		let a in 0..u32::MAX;
		let origin = T::GovernanceOrigin::successful_origin();
		let account_generation_key = a as u32;
		let proxy_account: T::AccountId = liquidity::<T>::generate_proxy_account(account_generation_key).unwrap();
		let main_account: T::AccountId = liquidity::<T>::generate_main_account(account_generation_key).unwrap();
		T::CallOcex::set_exchange_state_to_true()?;
	}: _(RawOrigin::Root, account_generation_key)
	verify {
		assert_last_event::<T>(Event::PalletAccountRegister {
			main_account , proxy_account
		}.into());
	}

	deposit_to_orderbook {
		let a in 1..255;
		let i in 0..u32::MAX;

		let origin = T::GovernanceOrigin::successful_origin();
		let asset = AssetId::asset(a.into());
		let amount = BalanceOf::<T>::decode(&mut &(a as u128).saturating_mul(10u128).to_le_bytes()[..]).unwrap();
		let account_generation_key = i as u32;
		let main_account: T::AccountId  = liquidity::<T>::generate_main_account(account_generation_key).unwrap();
		let proxy_account: T::AccountId = liquidity::<T>::generate_proxy_account(account_generation_key).unwrap();

		T::CallOcex::set_exchange_state_to_true()?;
		T::CallOcex::allowlist_and_create_token(main_account.clone(), a as u128)?;

		//register account
		let call = Call::<T>::register_account{account_generation_key};
		call.dispatch_bypass_filter(origin.clone())?;

		//existential deposit
		T::NativeCurrency::deposit_creating(
			&main_account.clone(),
			(10 * UNIT_BALANCE).saturated_into(),
		);

		let call = Call::<T>::deposit_to_orderbook {
		 asset, amount, account_generation_key
		};
	}: {call.dispatch_bypass_filter(origin)?}
	verify {
		assert_last_event::<T>(Event::DepositToPalletAccount {
			main_account , asset, amount
		}.into());
	}


	withdraw_from_orderbook {
		let a in 1..255;
		let i in 0..u32::MAX;

		let origin = T::GovernanceOrigin::successful_origin();
		let asset = AssetId::asset(a.into());
		let amount = BalanceOf::<T>::decode(&mut &(a as u128).saturating_mul(10u128).to_le_bytes()[..]).unwrap();
		let account_generation_key = i as u32;
		let main_account: T::AccountId  = liquidity::<T>::generate_main_account(account_generation_key).unwrap();
		let proxy_account: T::AccountId = liquidity::<T>::generate_proxy_account(account_generation_key).unwrap();
		let do_force_withdraw = true;
		T::CallOcex::set_exchange_state_to_true()?;
		T::CallOcex::allowlist_and_create_token(main_account.clone(), a as u128)?;

		//register account
		let call = Call::<T>::register_account{account_generation_key};
		call.dispatch_bypass_filter(origin.clone())?;

		//existential deposit
		T::NativeCurrency::deposit_creating(
			&main_account.clone(),
			(10 * UNIT_BALANCE).saturated_into(),
		);

		//deposit to orderbook
		let call = Call::<T>::deposit_to_orderbook {
		 asset, amount, account_generation_key
		};

		let call = Call::<T>::withdraw_from_orderbook {
		 asset, amount, do_force_withdraw, account_generation_key
		};

	}: {call.dispatch_bypass_filter(origin)?}

	verify {
		assert_last_event::<T>(Event::WithdrawFromPalletAccount {
			main_account , asset, amount
		}.into());
	}
}

#[cfg(test)]
impl_benchmark_test_suite!(liquidity, crate::mock::new_test_ext(), crate::mock::Test);

#[cfg(test)]
use frame_benchmarking::impl_benchmark_test_suite;
