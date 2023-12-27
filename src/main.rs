use libp2p::{gossipsub, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use std::collections::hash_map::DefaultHasher;
use std::{env, thread, u128};
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};
use libp2p::gossipsub::TopicScoreParams;
use tokio::{io, select, time};
use tracing_subscriber::EnvFilter;
use rand::prelude::*;
use tokio::net::lookup_host;
use libp2p::futures::StreamExt;
use gethostname::gethostname;
use libp2p::swarm::dial_opts::DialOpts;

// We create a custom network behaviour that combines Gossipsub.
#[derive(NetworkBehaviour)]
struct MyBehaviour {
    gossipsub: gossipsub::Behaviour
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| {
            // To content-address message, we can take the hash of message and use it as an ID.
            let message_id_fn = |message: &gossipsub::Message| {
                let mut s = DefaultHasher::new();
                message.data.hash(&mut s);
                gossipsub::MessageId::from(s.finish().to_string())
            };

            // Set a custom gossipsub configuration
            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .flood_publish(true)
                //.opportunistic_graft_ticks(-10000)
                .heartbeat_interval(Duration::from_secs(1))
                .prune_backoff(Duration::from_secs(60))
                .gossip_factor(0.25)
                .mesh_n(6)
                .mesh_n_low(4)
                .mesh_n_high(8)
                .retain_scores(6)
                .mesh_outbound_min(3)
                .gossip_lazy(6)
                .validation_mode(gossipsub::ValidationMode::Strict)
                .message_id_fn(message_id_fn) // content-address messages. No two messages of the same content will be propagated.
                .build()
                .map_err(|msg| io::Error::new(io::ErrorKind::Other, msg))?; // Temporary hack because `build` does not return a proper `std::error::Error`.

            // build a gossipsub network behaviour
            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )?;

            Ok(MyBehaviour { gossipsub })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    let mut params = TopicScoreParams::default();
    params.topic_weight = 1.0;
    params.first_message_deliveries_weight = 1.0;
    params.first_message_deliveries_cap = 3.0;
    params.first_message_deliveries_decay = 0.9;

    // Create a Gossipsub topic
    let topic = gossipsub::IdentTopic::new("test-net");
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

    let binding = gethostname();
    let hostname = binding.to_str();
    println!("Hostname: {:?}", hostname);

    // Listen on all interfaces and whatever port the OS assigns
    swarm.listen_on("/ip4/0.0.0.0/tcp/5000".parse()?)?;

    let my_id = env::var("PEERNUMBER").expect("$PEERNUMBER is not set");

    println!("{}, {}", my_id, swarm.local_peer_id().to_string());
    println!("Waiting 60 seconds for node building...");
    thread::sleep(Duration::from_secs(30));

    let peers = env::var("PEERS").expect("$PEERS is not set");
    let mut array: Vec<usize> = (0..peers.parse().unwrap()).collect();
    let mut rng = thread_rng();
    array.shuffle(&mut rng);

    let connect_to = env::var("CONNECTTO").expect("$CONNECTTO is not set");
    let mut connected = 0;
    for element in &array {
        if connected >= connect_to.parse().unwrap() { break; };
        if *element == my_id.parse::<usize>().unwrap() { continue; }
        let t_address = format!("pod-{}:5000", element);
        println!("Will connect to peer {element}");
        println!("Service: {element}");

        let mut addrs = String::from("/ip4/");

        loop {
            match lookup_host(&t_address).await {
                Ok(lookup_result) => {
                    for addr in lookup_result {
                        if addr.is_ipv4() {
                            println!("Resolved IPv4 address: {}", addr.ip());
                            addrs.push_str(addr.ip().to_string().as_str());
                            addrs.push_str("/tcp/5000");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to resolve address: {:?}", e);
                    println!("Waiting 15 seconds...");
                    thread::sleep(Duration::from_secs(15));
                    continue;
                }
            }
            break;
        }
        loop {
            println!("Trying to dial with {}", addrs);
            match swarm.dial(DialOpts::unknown_peer_id()
                                         .address(addrs.parse().unwrap())
                                         .build()) {
                Ok(..) => {
                    loop {
                        match swarm.select_next_some().await {
                            SwarmEvent::ConnectionEstablished{endpoint, ..} => {
                                if endpoint.is_dialer() {
                                    connected += 1;
                                    println!("Connected to {:?}!", endpoint);
                                    break;
                                }
                            }
                            swarm_event => {
                                println!("{:?}", swarm_event);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to dial: {:?}", e);
                    println!("Waiting 15 seconds...");
                    thread::sleep(Duration::from_secs(15));
                    continue;
                }
            }
            break;
        }
    }

    println!("Mesh size: {:?}", swarm.network_info().connection_counters());

    let turn_to_publish = my_id.parse::<i32>().unwrap();
    println!("Publishing turn is: {:?}", turn_to_publish);

    println!("Waiting 30 for connections...");
    thread::sleep(Duration::from_secs(30));

    let rate = env::var("MSGRATE").expect("$MSGRATE is not set").parse().unwrap();
    let mut interval = time::interval(Duration::from_millis(rate));
    let msg_size = env::var("MSGSIZE").expect("$MSGSIZE is not set").parse().unwrap();
    let int_peers: i32 = peers.parse().unwrap();
    let mut counter = 0;

    loop {
        select! {
            _ =  interval.tick() => {
                if counter % int_peers == turn_to_publish {
                    let duration_since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
                    let timestamp_nanos = duration_since_epoch.as_nanos();
                    // Convert the timestamp to little-endian bytes
                    let now_bytes: [u8; 16] = u128::to_le_bytes(timestamp_nanos as u128);
                    // Create a new vector of bytes for the message
                    let new_seq: Vec<u8> = vec![0; msg_size];
                    // Concatenate the little-endian bytes and the new sequence
                    let mut now_bytes_vec = Vec::new();
                    now_bytes_vec.extend_from_slice(&now_bytes);
                    now_bytes_vec.extend_from_slice(&new_seq);

                    let result = swarm
                        .behaviour_mut().gossipsub
                        .publish(gossipsub::IdentTopic::new("test-net"), now_bytes_vec);

                    match result {
                        Ok(message_id) => {
                            println!("Sent: {}, id: {}", timestamp_nanos, message_id);
                        }
                        Err(error) => {
                            println!("Publish is Err: {}", error);
                        }
                    }
                }
                counter += 1;
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => {
                        let sent_moment = u128::from_le_bytes(message.data[..16].try_into().unwrap());
                        let duration_since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
                        let timestamp_nanos = duration_since_epoch.as_nanos();
                        let diff = timestamp_nanos - sent_moment;
                        println!("{}, milliseconds: {}", sent_moment, Duration::from_nanos(diff as u64).as_millis());
                    }
                _ => {}
            }
        }
    }
}