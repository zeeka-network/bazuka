#[cfg(test)]
mod test;

mod api;
mod context;
mod heartbeat;
mod http;
pub mod seeds;
pub mod upnp;
use context::NodeContext;

use crate::blockchain::Blockchain;
use crate::client::{
    Limit, NodeError, NodeRequest, OutgoingSender, Peer, PeerAddress, PeerInfo, Timestamp,
};
use crate::crypto::ed25519;
use crate::crypto::SignatureScheme;
use crate::wallet::Wallet;
use hyper::body::HttpBody;
use hyper::header::AUTHORIZATION;
use hyper::{Body, Method, Request, Response, StatusCode};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::try_join;

#[derive(Debug, Clone)]
pub struct NodeOptions {
    pub heartbeat_interval: Duration,
    pub num_peers: usize,
    pub outdated_heights_threshold: u32,
    pub no_response_punish: u32,
    pub invalid_data_punish: u32,
    pub incorrect_power_punish: u32,
    pub max_punish: u32,
    pub state_unavailable_ban_time: u32,
}

fn fetch_signature(
    req: &Request<Body>,
) -> Result<Option<(ed25519::PublicKey, ed25519::Signature)>, NodeError> {
    if let Some(v) = req.headers().get(AUTHORIZATION) {
        let s = v.to_str().map_err(|_| NodeError::InvalidSignatureHeader)?;
        let mut s = s.split('-');
        let (pub_hex, sig_hex) = s
            .next()
            .zip(s.next())
            .ok_or(NodeError::InvalidSignatureHeader)?;
        let pub_key = hex::decode(pub_hex)
            .map(|bytes| bincode::deserialize::<ed25519::PublicKey>(&bytes))
            .map_err(|_| NodeError::InvalidSignatureHeader)?
            .map_err(|_| NodeError::InvalidSignatureHeader)?;
        let sig = hex::decode(sig_hex)
            .map(|bytes| bincode::deserialize::<ed25519::Signature>(&bytes))
            .map_err(|_| NodeError::InvalidSignatureHeader)?
            .map_err(|_| NodeError::InvalidSignatureHeader)?;
        return Ok(Some((pub_key, sig)));
    }
    Ok(None)
}

async fn node_service<B: Blockchain>(
    _client: Option<SocketAddr>,
    context: Arc<RwLock<NodeContext<B>>>,
    req: Request<Body>,
) -> Result<Response<Body>, NodeError> {
    let mut response = Response::new(Body::empty());
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let qs = req.uri().query().unwrap_or("").to_string();

    let creds = fetch_signature(&req)?;
    let body = req.into_body();

    // Disallow large requests
    if body
        .size_hint()
        .upper()
        .map(|u| u > 1024 * 1024)
        .unwrap_or(true)
    {
        *response.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
        return Ok(response);
    }

    let body_bytes = hyper::body::to_bytes(body).await?;

    let needs_signature = false;

    // TODO: This doesn't prevent replay attacks
    let is_signed = creds
        .map(|(pub_key, sig)| {
            ed25519::Ed25519::<crate::core::Hasher>::verify(&pub_key, &body_bytes, &sig)
        })
        .unwrap_or(false);
    if needs_signature && !is_signed {
        return Err(NodeError::SignatureRequired);
    }

    match (method, &path[..]) {
        // Miner will call this to fetch new PoW work.
        (Method::GET, "/miner/puzzle") => {
            *response.body_mut() = Body::from(serde_json::to_vec(
                &api::get_miner_puzzle(Arc::clone(&context), serde_qs::from_str(&qs)?).await?,
            )?);
        }

        // Miner will call this when he has solved the PoW puzzle.
        (Method::POST, "/miner/solution") => {
            *response.body_mut() = Body::from(serde_json::to_vec(
                &api::post_miner_solution(
                    Arc::clone(&context),
                    serde_json::from_slice(&body_bytes)?,
                )
                .await?,
            )?);
        }

        (Method::GET, "/stats") => {
            *response.body_mut() = Body::from(serde_json::to_vec(
                &api::get_stats(Arc::clone(&context), serde_qs::from_str(&qs)?).await?,
            )?);
        }
        (Method::GET, "/account") => {
            *response.body_mut() = Body::from(serde_json::to_vec(
                &api::get_account(Arc::clone(&context), serde_qs::from_str(&qs)?).await?,
            )?);
        }
        (Method::GET, "/peers") => {
            *response.body_mut() = Body::from(serde_json::to_vec(
                &api::get_peers(Arc::clone(&context), serde_qs::from_str(&qs)?).await?,
            )?);
        }
        (Method::POST, "/peers") => {
            *response.body_mut() = Body::from(serde_json::to_vec(
                &api::post_peer(Arc::clone(&context), serde_json::from_slice(&body_bytes)?).await?,
            )?);
        }
        (Method::POST, "/shutdown") => {
            *response.body_mut() = Body::from(serde_json::to_vec(
                &api::shutdown(Arc::clone(&context), serde_json::from_slice(&body_bytes)?).await?,
            )?);
        }
        (Method::POST, "/bincode/transact") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::transact(Arc::clone(&context), bincode::deserialize(&body_bytes)?).await?,
            )?);
        }
        (Method::POST, "/bincode/transact/zero") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::transact_zero(Arc::clone(&context), bincode::deserialize(&body_bytes)?)
                    .await?,
            )?);
        }
        (Method::POST, "/bincode/transact/dw") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::transact_deposit_withdraw(
                    Arc::clone(&context),
                    bincode::deserialize(&body_bytes)?,
                )
                .await?,
            )?);
        }
        (Method::GET, "/bincode/headers") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::get_headers(Arc::clone(&context), bincode::deserialize(&body_bytes)?).await?,
            )?);
        }
        (Method::GET, "/bincode/blocks") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::get_blocks(Arc::clone(&context), bincode::deserialize(&body_bytes)?).await?,
            )?);
        }
        (Method::POST, "/bincode/blocks") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::post_block(Arc::clone(&context), bincode::deserialize(&body_bytes)?).await?,
            )?);
        }
        (Method::GET, "/bincode/states") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::get_states(Arc::clone(&context), bincode::deserialize(&body_bytes)?).await?,
            )?);
        }
        (Method::GET, "/bincode/states/outdated") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::get_outdated_heights(
                    Arc::clone(&context),
                    bincode::deserialize(&body_bytes)?,
                )
                .await?,
            )?);
        }
        (Method::GET, "/bincode/mempool/zero") => {
            *response.body_mut() = Body::from(bincode::serialize(
                &api::get_zero_mempool(Arc::clone(&context), bincode::deserialize(&body_bytes)?)
                    .await?,
            )?);
        }
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
        }
    };

    Ok(response)
}

use tokio::sync::mpsc;

pub async fn node_create<B: Blockchain>(
    opts: NodeOptions,
    address: PeerAddress,
    priv_key: ed25519::PrivateKey,
    bootstrap: Vec<PeerAddress>,
    blockchain: B,
    timestamp_offset: i32,
    wallet: Option<Wallet>,
    mut incoming: mpsc::UnboundedReceiver<NodeRequest>,
    outgoing: mpsc::UnboundedSender<NodeRequest>,
) -> Result<(), NodeError> {
    let context = Arc::new(RwLock::new(NodeContext {
        opts,
        address,
        pub_key: ed25519::PublicKey::from(priv_key.clone()),
        shutdown: false,
        outgoing: Arc::new(OutgoingSender {
            chan: outgoing,
            priv_key,
        }),
        blockchain,
        wallet,
        mempool: HashMap::new(),
        zero_mempool: HashMap::new(),
        dw_mempool: HashMap::new(),
        peers: bootstrap
            .into_iter()
            .map(|addr| {
                (
                    addr,
                    Peer {
                        pub_key: None,
                        address: addr,
                        punished_until: 0,
                        info: None,
                    },
                )
            })
            .collect(),
        timestamp_offset,
        banned_headers: HashMap::new(),
        outdated_since: None,

        miner_puzzle: None,
    }));

    let server_future = async {
        loop {
            if context.read().await.shutdown {
                break;
            }
            if let Some(msg) = incoming.recv().await {
                if let Err(e) = msg
                    .resp
                    .send(node_service(msg.socket_addr, Arc::clone(&context), msg.body).await)
                    .await
                {
                    log::error!("Request sender not receiving its answer: {}", e);
                }
            } else {
                break;
            }
        }
        Ok(())
    };

    let heartbeat_future = heartbeat::heartbeater(Arc::clone(&context));

    try_join!(server_future, heartbeat_future)?;

    log::info!("Node stopped!");

    Ok(())
}
