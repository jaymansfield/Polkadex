use crate::{
	error::Error,
	gossip,
	gossip::{topic, GossipValidator},
	metrics::Metrics,
	Client,
};
use futures::{channel::mpsc::UnboundedReceiver, StreamExt};
use log::{debug, error, info, trace, warn};
use memory_db::{HashKey, MemoryDB};
use orderbook_primitives::{types::ObMessage, ObApi, StidImportRequest};
use parity_scale_codec::{Codec, Decode, Encode};
use parking_lot::RwLock;
use sc_client_api::Backend;
use sc_network_common::service::{NotificationSender, NotificationSenderError};
use sc_network_gossip::GossipEngine;
use sp_api::ProvideRuntimeApi;
use sp_consensus::SyncOracle;
use sp_runtime::{generic::BlockId, traits::Block};
use std::{
	borrow::Cow,
	collections::{BTreeMap, HashMap},
	marker::PhantomData,
	sync::Arc,
};
use sc_network_common::protocol::event::Event;

pub(crate) struct WorkerParams<B: Block, BE, C, SO, N> {
	pub client: Arc<C>,
	pub backend: Arc<BE>,
	pub sync_oracle: SO,
	// pub key_store: BeefyKeystore,
	// pub links: BeefyVoterLinks<B>,
	pub metrics: Option<Metrics>,
	pub message_sender_link: UnboundedReceiver<ObMessage>,
	/// Gossip network
	pub network: N,
	/// Chain specific Ob protocol name. See [`orderbook_protocol_name::standard_name`].
	pub protocol_name: std::borrow::Cow<'static, str>,
	pub _marker: PhantomData<B>,
}

/// A Orderbook worker plays the Orderbook protocol
pub(crate) struct ObWorker<B: Block, BE, C, SO, N> {
	// utilities
	client: Arc<C>,
	backend: Arc<BE>,
	sync_oracle: SO,
	network: N,
	// key_store: BeefyKeystore,
	gossip_engine: GossipEngine<B>,
	gossip_validator: Arc<GossipValidator<B>>,
	// Last processed state change id
	last_processed_stid: Arc<RwLock<u64>>,
	// Known state ids
	known_messages: BTreeMap<u64, ObMessage>,
	// Links between the block importer, the background voter and the RPC layer.
	// links: BeefyVoterLinks<B>,

	// voter state
	/// Orderbook client metrics.
	metrics: Option<Metrics>,
	message_sender_link: UnboundedReceiver<ObMessage>,
	_marker: PhantomData<N>,
	// In memory store
	memory_db: MemoryDB<CustomBlake2Hasher, HashKey<CustomBlake2Hasher>, Vec<u8>>,
}
use crate::hasher::CustomBlake2Hasher;
use orderbook_primitives::types::UserActions;
use sc_network_gossip::Network as GossipNetwork;
use sp_arithmetic::traits::Saturating;
use sp_core::{offchain::OffchainStorage, H256};

impl<B, BE, C, SO, N> ObWorker<B, BE, C, SO, N>
where
	B: Block + Codec,
	BE: Backend<B>,
	C: Client<B, BE> + ProvideRuntimeApi<B>,
	C::Api: ObApi<B>,
	SO: Send + Sync + Clone + 'static + SyncOracle,
	N: GossipNetwork<B> + Clone + Send + Sync + 'static,
{
	/// Return a new BEEFY worker instance.
	///
	/// Note that a BEEFY worker is only fully functional if a corresponding
	/// BEEFY pallet has been deployed on-chain.
	///
	/// The BEEFY pallet is needed in order to keep track of the BEEFY authority set.
	pub(crate) fn new(worker_params: WorkerParams<B, BE, C, SO, N>) -> Self {
		let WorkerParams {
			client,
			backend,
			// key_store,
			sync_oracle,
			// links,
			metrics,
			message_sender_link,
			network,
			protocol_name,
			_marker,
		} = worker_params;

		let last_processed_stid = Arc::new(RwLock::new(0));
		let gossip_validator = Arc::new(GossipValidator::new(last_processed_stid.clone()));
		let gossip_engine =
			GossipEngine::new(network.clone(), protocol_name, gossip_validator.clone(), None);

		ObWorker {
			client: client.clone(),
			backend,
			sync_oracle,
			// key_store,
			network,
			gossip_engine,
			gossip_validator,
			memory_db: MemoryDB::default(),
			// links,
			// last_processed_state_change_id,
			message_sender_link,
			metrics,
			last_processed_stid,
			_marker: Default::default(),
			known_messages: Default::default(),
		}
	}

	pub fn handle_action(&mut self, action: ObMessage) -> Result<(), Error> {
		// TODO: All user logic goes here
		todo!()
	}

	// Checks if we need to sync the orderbook state before processing the messages.
	pub async fn check_state_sync(&mut self) -> Result<(), Error> {
		// Read latest snapshot from finalizized state
		let summary = self
			.client
			.runtime_api()
			.get_latest_snapshot(&BlockId::number(self.client.info().finalized_number))
			.expect("Something went wrong with the get_latest_snapshot runtime api; qed.");

		// We need to sync only if we are need to update state
		if *self.last_processed_stid.read() < summary.state_change_id {
			// Try to load it from our local DB if not download it from Orderbook operator
			if let Err(err) = self.load_snapshot(summary.state_change_id, summary.state_hash) {
				info!(target: "orderbook", "📒 Orderbook state data not found locally for stid: {:?}",summary.state_change_id);
				self.download_snapshot_from_operator(summary.state_change_id, summary.state_hash)?;
			}
			// X->Y sync: Ask peers to send the missed stid
			if !self.known_messages.is_empty() {
				// Collect all known stids
				let mut known_stids = self.known_messages.keys().collect::<Vec<&u64>>();
				known_stids.sort_unstable(); // unstable is fine since we know stids are unique
							 // if the next best known stid is not available then ask others
				if *known_stids[0] != (*self.last_processed_stid.read()).saturating_add(1) {
					// Ask other peers to send us the requests stids.
					let import_request = StidImportRequest {
						from: *self.last_processed_stid.read(),
						to: *known_stids[0],
					};

					for peer in &self.gossip_validator.peers {
						match self.network.notification_sender(*peer, Cow::from("stid_request")) {
							Ok(sender) => {
								if let Ok(mut s) = sender.ready().await {
									// Send the request and exit and wait for peers to send back the
									// information
									if let Err(err) = s.send(import_request.encode()) {
										error!(target: "orderbook", "📒 error while sending notif to {:?} for missing stid: {:?}",peer,err)
									}
								}
							},
							Err(err) =>
								error!(target: "orderbook", "📒 error while requesting {:?} for missing stid: {:?}",peer,err),
						}
					}
				} else {
					info!(target: "orderbook", "📒 sync request not required, we know the next stid");
				}
			} else {
				info!(target: "orderbook", "📒 No new messages known after stid: {:?}",*self.last_processed_stid.read());
			}
		} else {
			info!(target: "orderbook", "📒 Sync is not required latest stid: {:?}, last_snapshot_stid: {:?}",*self.last_processed_stid.read(), summary.state_change_id);
		}
		Ok(())
	}

	pub fn download_snapshot_from_operator(
		&mut self,
		stid: u64,
		expected_state_hash: H256,
	) -> Result<(), Error> {
		todo!();

		// let computed_hash = sp_core::blake2_256(&data);
		// if computed_hash != expected_state_hash {
		// 	warn!(target:"orderbook","📒 orderbook state hash mismatch: computed: {:?}, expected:
		// {:?}",computed_hash,expected_state_hash); 	return Err(Error::StateHashMisMatch)
		// }
		//
		// *self.last_processed_stid.write() = stid;
		Ok(())
	}

	pub async fn process_new_user_action(&mut self, action: &ObMessage) -> Result<(), Error> {
		// Cache the message
		self.known_messages.insert(action.stid, action.clone());
		if self.sync_oracle.is_major_syncing() {
			info!(target: "orderbook", "📒 Ob message cached for sync to complete: stid: {:?}",action.stid);
			return Ok(())
		}
		self.check_state_sync().await?;
		let mut last_processed_stid = (*self.last_processed_stid.read()).saturating_add(1);

		while let Some(action) = self.known_messages.remove(&last_processed_stid) {
			self.handle_action(action)?;
			last_processed_stid = last_processed_stid.saturating_add(1);
		}
		// We need to sub 1 since that last processed is one stid less than the not available
		// when while loop is broken
		*self.last_processed_stid.write() = last_processed_stid.saturating_sub(1);

		Ok(())
	}

	pub fn handle_gossip_message(&mut self, message: &ObMessage) -> Result<(), Error> {
		todo!()
	}

	pub fn store_snapshot(&mut self, snapshot_id: u64) -> Result<(), Error> {
		if let Some(mut offchain_storage) = self.backend.offchain_storage() {
			match serde_json::to_vec(self.memory_db.data()) {
				Ok(data) => offchain_storage.set(
					b"OrderbookSnapshotState",
					&snapshot_id.to_le_bytes(),
					&data,
				),
				Err(err) =>
					return Err(Error::Backend(format!("Error serializing the data: {:?}", err))),
			}
		}
		return Err(Error::Backend("Offchain Storage not Found".parse().unwrap()))
	}

	pub fn load_snapshot(
		&mut self,
		snapshot_id: u64,
		expected_state_hash: H256,
	) -> Result<(), Error> {
		if let Some(offchain_storage) = self.backend.offchain_storage() {
			if let Some(mut data) =
				offchain_storage.get(b"OrderbookSnapshotState", &snapshot_id.to_le_bytes())
			{
				let computed_hash = H256::from(sp_core::blake2_256(&data));
				if computed_hash != expected_state_hash {
					warn!(target:"orderbook","📒 orderbook state hash mismatch: computed: {:?}, expected: {:?}",computed_hash,expected_state_hash);
					return Err(Error::StateHashMisMatch)
				}

				match serde_json::from_slice::<HashMap<H256, (Vec<u8>, i32)>>(&data) {
					Ok(data) => {
						self.memory_db.load_from(data);
						*self.last_processed_stid.write() = snapshot_id;
					},
					Err(err) =>
						return Err(Error::Backend(format!(
							"Error decoding snapshot data: {:?}",
							err
						))),
				}
			}
		}
		Ok(())
	}

	pub async fn handle_network_event(&self, event: &Event) -> Result<(), Error> {
		todo!()
	}

	/// Main loop for Orderbook worker.
	///
	/// Wait for Orderbook runtime pallet to be available, then start the main async loop
	/// which is driven by gossiped user actions.
	pub(crate) async fn run(mut self) {
		info!(target: "orderbook", "📒 Orderbook worker started");

		let mut gossip_messages = Box::pin(
			self.gossip_engine
				.messages_for(topic::<B>())
				.filter_map(|notification| async move {
					trace!(target: "orderbook", "📒 Got gossip message: {:?}", notification);

					ObMessage::decode(&mut &notification.message[..]).ok()
				})
				.fuse(),
		);

		let mut notification_events_stream = self.network.event_stream("orderbook").fuse();

		loop {
			let mut gossip_engine = &mut self.gossip_engine;
			futures::select_biased! {
				gossip = gossip_messages.next() => {
					if let Some(message) = gossip {
						// Gossip messages have already been verified to be valid by the gossip validator.
						if let Err(err) = self.handle_gossip_message(&message) {
							debug!(target: "orderbook", "📒 {}", err);
						}
					} else {
						return;
					}
				},
				message = self.message_sender_link.next() => {
					if let Some(message) = message {
						if let Err(err) = self.process_new_user_action(&message).await {
							debug!(target: "orderbook", "📒 {}", err);
						}
					}else{
						return;
					}
				},
				notification = notification_events_stream.next() => {

					if let Some(notification) = notification {
					if let Err(err) = self.handle_network_event(&notification).await {
							debug!(target: "orderbook", "📒 {}", err);
					}
					}else {
						error!(target:"orderbook","None notification recvd");
						return
					}
				},
				_ = gossip_engine => {
					error!(target: "orderbook", "📒 Gossip engine has terminated.");
					return;
				}
			}
		}
	}
}
