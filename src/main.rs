use libp2p::{gossipsub, Multiaddr, noise, swarm::NetworkBehaviour, swarm::SwarmEvent, tcp, yamux};
use std::collections::hash_map::DefaultHasher;
use std::{env, thread};
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};
use libp2p::gossipsub::TopicScoreParams;
use tokio::{io, select, time};
use tracing_subscriber::EnvFilter;
use rand::prelude::*;
use chrono::prelude::*;
use tokio::net::lookup_host;
use libp2p::futures::StreamExt;
use gethostname::gethostname;

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
                .heartbeat_interval(Duration::from_secs(1)) // This is set to aid debugging by not cluttering the log space
                .prune_backoff(Duration::from_secs(60))
                .gossip_factor(0.25)
                .mesh_n(6)
                .mesh_n_low(4)
                .mesh_n_high(8)
                .retain_scores(6)
                .mesh_outbound_min(3)
                .gossip_lazy(6)
                .validation_mode(gossipsub::ValidationMode::Strict) // This sets the kind of message validation. The default is Strict (enforce message signing)
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

    // Listen on all interfaces and whatever port the OS assigns
    swarm.listen_on("/ip4/0.0.0.0/tcp/5000".parse()?)?;

    let my_id = env::var("PEERNUMBER").expect("$PEERNUMBER is not set");

    println!("{}, {}", my_id, swarm.local_peer_id().to_string());
    println!("Waiting 60 seconds for node building...");
    thread::sleep(Duration::from_secs(60));

    let peers = env::var("PEERS").expect("$PEERS is not set");
    let mut array: Vec<usize> = (0..peers.parse().unwrap()).collect();
    let mut rng = thread_rng();
    array.shuffle(&mut rng);

    let connect_to = env::var("CONNECTTO").expect("$CONNECTTO is not set");
    let mut connected = 0;
    for element in &array {
        if connected >= connect_to.parse().unwrap() { break };
        let t_address = format!("pod-{}:5000", element);
        println!("Will connect to peer {element}");
        println!("Service: {element}");

        let mut addrs = String::new();

        loop {
            match lookup_host(&t_address).await {
                Ok(lookup_result) => {
                    for addr in lookup_result {
                        if addr.is_ipv4() {
                            println!("Resolved IPv4 address: {}", addr.ip());
                            addrs.push(format!("{}:5000", addr.ip().to_string()).as_str().parse().unwrap());
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to resolve address: {:?}", e);
                    println!("Waiting 15 seconds...");
                    thread::sleep(Duration::from_secs(15));
                    continue;
                },
            }
            break;
        }

        loop {
            match swarm.dial(addrs.parse::<Multiaddr>().unwrap()) {
                Ok(..) => {
                    connected += 1;
                    println!("Connected!");
                }
                Err(e) => {
                    eprintln!("Failed to dial: {:?}", e);
                    println!("Waiting 15 seconds...");
                    thread::sleep(Duration::from_secs(15));
                    continue;
                },
            }
            break;
        }
    }

    println!("Mesh size: {}", swarm.network_info().num_peers());
    let binding = gethostname();
    let hostname = binding.to_str();
    println!("Hostname: {:?}", hostname);

    let turn_to_publish: i32 = hostname.expect("No hostname").trim_start_matches("pod-").parse().unwrap();
    println!("Publishing turn is: {:?}", turn_to_publish);

    let mut interval = time::interval(Duration::from_millis(1000));

    loop {
        select! {
            _ =  interval.tick() => {
                let msg_size = 1000;
                let int_peers: i32 = peers.parse().unwrap();
                for msg in 0..10000 {
                    if msg % int_peers.clone() == turn_to_publish {
                        let datetime: DateTime<Utc> = SystemTime::now().into();
                        println!("Sending message at: {}", datetime.format("%d/%m/%Y %T"));

                        let duration_since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
                        let timestamp_nanos = duration_since_epoch.as_nanos();
                        let now_bytes = timestamp_nanos.to_le_bytes();

                        let mut now_bytes_vec = Vec::from(&now_bytes[..]);
                        now_bytes_vec.resize(msg_size.clone(), 0);

                        let _ = swarm
                            .behaviour_mut().gossipsub
                            .publish(gossipsub::IdentTopic::new("test-net"), now_bytes_vec);
                    }
                }
            }
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(MyBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                    propagation_source: peer_id,
                    message_id: id,
                    message,
                })) => println!(
                        "Got message: '{}' with id: {id} from peer: {peer_id}",
                        String::from_utf8_lossy(&message.data),
                ),
                _ => {}
            }
        }
    }
}