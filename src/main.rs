use chrono::prelude::*;
use libp2p::{
    core::upgrade,
    futures::StreamExt,
    mplex,
    noise::{Keypair, NoiseConfig, X25519Spec},
    swarm::{Swarm, SwarmBuilder, SwarmEvent},
    tcp::TokioTcpConfig,
    Transport, 
    mdns::MdnsEvent,
};
use log::{error, info};
use serde::{Deserialize, Serialize};
// use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{
    io::{stdin, AsyncBufReadExt, BufReader},
    select, spawn,
    sync::mpsc,
    time::sleep,
};

use crate::p2p::{MyBehaviourEvent};

// const DIFFICULTY_PREFIX: &str = "00";

mod p2p;

pub struct App {
    pub blocks: Vec<Block>,
}

impl App {
    fn new() -> Self {
        Self { blocks: vec![] }
    }

    fn genesis(&mut self) {
        let genesis_block = Block {
            id: 0,
            timestamp: Utc::now().timestamp(),
            previous_hash: String::from("genesis"),
            data: String::from("genesis!"),
        };
        self.blocks.push(genesis_block);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Block {
    pub id: u64,
    // pub hash: String,
    pub previous_hash: String,
    pub timestamp: i64,
    pub data: String,
    // pub nonce: u64,
}

impl Block {
    pub fn new(id: u64, previous_hash: String, data: String) -> Self {
        let now = Utc::now();
        // let (nonce, hash) = mine_block(id, now.timestamp(), &previous_hash, &data);
        Self {
            id,
            // hash,
            timestamp: now.timestamp(),
            previous_hash,
            data,
            // nonce,
        }
    }
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    info!("Peer Id: {}", p2p::PEER_ID.clone());
    let (response_sender, mut response_rcv) = mpsc::unbounded_channel();
    let (init_sender, mut init_rcv) = mpsc::unbounded_channel();

    let auth_keys = Keypair::<X25519Spec>::new()
        .into_authentic(&p2p::KEYS)
        .expect("can create auth keys");

    let transp = TokioTcpConfig::new()
        .upgrade(upgrade::Version::V1)
        .authenticate(NoiseConfig::xx(auth_keys).into_authenticated())
        .multiplex(mplex::MplexConfig::new())
        .boxed();

    let behaviour = p2p::AppBehaviour::new(App::new(), response_sender, init_sender.clone()).await;

    // let sub = p2p::AppBehaviour::sub(&mut behaviour);

    let mut swarm = SwarmBuilder::new(transp, behaviour, *p2p::PEER_ID)
        .executor(Box::new(|fut| {
            spawn(fut);
        }))
        .build();

    let mut stdin = BufReader::new(stdin()).lines();

    Swarm::listen_on(
        &mut swarm,
        "/ip4/0.0.0.0/tcp/0"
            .parse()
            .expect("can get a local socket"),
    )
    .expect("swarm can be started");
    println!("{:?}", init_sender);
    spawn(async move {
        sleep(Duration::from_secs(1)).await;
        info!("sending init event");
        init_sender.send(true).expect("can send init event");
    });

    loop {
        let evt = {
            select! {
                line = stdin.next_line() => Some(p2p::EventType::Input(line.expect("can get line").expect("can read line from stdin"))),
                response = response_rcv.recv() => {
                    Some(p2p::EventType::LocalChainResponse(response.expect("response exists")))
                },
                _ = init_rcv.recv() => {
                    Some(p2p::EventType::Init)
                }
                event = swarm.select_next_some() => {
                    // info!("Unhandled Swarm Event: {:?}", event);
                    match event {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            // println!("Listening on {:?}", address);
                            Some(p2p::EventType::Idle(address))
                        }
                        SwarmEvent::IncomingConnection { local_addr, send_back_addr } => {
                            // println!(
                            Some(p2p::EventType::IncomingConnection(local_addr, send_back_addr))
                            //         "Received: '{:?}' from {:?}",
                            //         String::from_utf8_lossy(&message.data),
                            //         message.source
                            //     );
                        }
                        // SwarmEvent::Behaviour(AppBehaviour { floodsub, ..}) => {
                        //     swarm.behaviour_mut().floodsub.subscribe(p2p::CHAIN_TOPIC.clone());
                        //     // behaviour.floodsub.subscribe(floodsub_topic.clone());
                        //     // println!("{} as{:?}", peer_id, topic);
                        // }
                        SwarmEvent::Behaviour(MyBehaviourEvent::Mdns(tt)) => {
                            match tt {
                                MdnsEvent::Discovered(_list) => {
                                    // for (peer, s) in list {
                                    //     swarm.behaviour_mut().floodsub.add_node_to_partial_view(peer);
                                    //     println!("{:?}", s);
                                    // }
                                    Some(p2p::EventType::Discovered)
                                }
                                MdnsEvent::Expired(_list) => {
                                    // for (peer, _) in list {
                                    //     if !swarm.behaviour().mdns.has_node(&peer) {
                                    //         swarm.behaviour_mut().floodsub.remove_node_from_partial_view(&peer);
                                    //     }
                                    // }
                                    Some(p2p::EventType::Expired)
                                }
                            }
                        }
                        _ => { None }
                    }
                },
            }
        };

        if let Some(event) = evt {
            match event {
                p2p::EventType::Idle(address) => info!("idle {}",address),
                p2p::EventType::Expired => println!("la"),
                p2p::EventType::Discovered => info!("la"),
                p2p::EventType::IncomingConnection(local, send) => info!("ici {} send {}", local, send),
                p2p::EventType::Init => {
                    let peers = p2p::get_list_peers(&swarm);
                    swarm.behaviour_mut().app.genesis();

                    info!("connected nodes: {}", peers.len());
                    if !peers.is_empty() {
                        let req = p2p::LocalChainRequest {
                            from_peer_id: peers
                                .iter()
                                .last()
                                .expect("at least one peer")
                                .to_string(),
                        };

                        let json = serde_json::to_string(&req).expect("can jsonify request");
                        swarm
                            .behaviour_mut()
                            .floodsub
                            .publish(p2p::CHAIN_TOPIC.clone(), json.as_bytes());
                    }
                }
                p2p::EventType::LocalChainResponse(resp) => {
                    let json = serde_json::to_string(&resp).expect("can jsonify response");
                    swarm
                        .behaviour_mut()
                        .floodsub
                        .publish(p2p::CHAIN_TOPIC.clone(), json.as_bytes());
                }
                p2p::EventType::Input(line) => match line.as_str() {
                    "ls p" => p2p::handle_print_peers(&swarm),
                    cmd if cmd.starts_with("ls c") => p2p::handle_print_chain(&swarm),
                    cmd if cmd.starts_with("create b") => p2p::handle_create_block(cmd, &mut swarm),
                    _ => error!("unknown command"),
                },
            }
        }
    }
}
