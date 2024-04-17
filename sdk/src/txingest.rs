use {
    bincode::Options,
    crossbeam_channel::{bounded, Receiver, Sender},
    solana_sdk::signature::Signature,
    std::{
        io::Write,
        net::{IpAddr, SocketAddr, TcpStream},
        sync::OnceLock,
        time::{SystemTime, UNIX_EPOCH},
    },
};

#[derive(Serialize, Deserialize)]
// timestamp in all messages is number of milliseconds since Unix Epoch
pub enum TxIngestMsg {
    // Failed to accept a new QUIC connection from the remote peer because the number of QUIC connections has already
    // been exceeded
    Exceeded {
        timestamp: u64,
        peer_addr: SocketAddr,
    },
    // Failed to accept a new QUIC connection from the remote peer because max_connections is 0
    Disallowed {
        timestamp: u64,
        peer_addr: SocketAddr,
    },
    // Failed to accept a new QUIC connection from the remote peer because too many
    TooMany {
        timestamp: u64,
        peer_addr: SocketAddr,
    },
    // Issued when a QUIC connection has been fully established -- at this point the stake of the remote peer is known
    Stake {
        timestamp: u64,
        peer_addr: SocketAddr,
        stake: u64,
    },
    // A previously established QUIC connection has been closed by the local peer, typically because it has been
    // pruned
    Dropped {
        timestamp: u64,
        peer_addr: SocketAddr,
    },
    // A previously established QUIC connection has been closed by some action or inaction of the peer: either
    // an error by the remote peer, or an explicit close, or a timeout beacuse the remote peer didn't send packets.
    Closed {
        timestamp: u64,
        peer_addr: SocketAddr,
    },
    // A simple vote was received from the remote peer
    VoteTx {
        timestamp: u64,
        peer_addr: IpAddr,
        peer_port: u16,
    },
    // A user tx was received from the remote peer
    UserTx {
        timestamp: u64,
        peer_addr: IpAddr,
        peer_port: u16,
        signature: Signature,
    },
    // A tx was forwarded to a remote leader
    Forwarded {
        timestamp: u64,
        signature: Signature,
    },
    // A tx was not able to pay its fee
    BadFee {
        timestamp: u64,
        signature: Signature,
    },
    // A tx was executed and paid a fee
    Fee {
        timestamp: u64,
        signature: Signature,
        fee: u64,
    },
}

// Logs an error and does nothing for all calls beyond the first
pub fn txingest_connect(addr: &str) {
    if TX_INGEST.get().is_some() {
        log::error!("txingest_connect called more than once");
        return;
    }

    {
        let (sender, receiver) = bounded::<TxIngestMsg>(MAX_BUFFERED_TXINGEST_MESSAGES);

        match TX_INGEST.set(TxIngest {
            sender,
            receiver,
            default_options: bincode::DefaultOptions::new(),
        }) {
            Ok(_) => (),
            Err(_) => panic!("Failed to create TX_INGEST"),
        }
    }

    let addr = addr.to_string();

    std::thread::spawn(move || {
        loop {
            // Make a connection
            let mut tcp_stream = loop {
                match TcpStream::connect(&addr) {
                    Ok(tcp_stream) => break tcp_stream,
                    Err(e) => {
                        log::warn!(
                            "Failed to connect to txingest peer {addr} because {e}, trying again in 1 second"
                        );
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
            };
            loop {
                // Read message from TxIngest receiver and write it to tcp_stream; if error, break the loop which will
                // create a new tcp_stream
                let tx_ingest = TX_INGEST.get().expect("txingest channel failure (1)");
                let tx_ingest_msg = tx_ingest
                    .receiver
                    .recv()
                    .expect("txingest channel failure (2)");

                match tx_ingest.default_options.serialize(&tx_ingest_msg) {
                    Ok(vec_bytes) => match tcp_stream.write_all(&vec_bytes) {
                        Ok(_) => (),
                        Err(e) => {
                            log::warn!("txingest connection failed {e}, re-connecting");
                            break;
                        }
                    },
                    Err(e) => log::error!("Failed to serialize txingest message because {e}"),
                }
            }
        }
    });
}

// Convenience for getting a unix timestamp
pub fn txingest_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}

// Send TxIngestMsg to connected peer.  Message will be dropped if txingest_connect() has not been called yet or the
// channel is full
pub fn txingest_send(msg: TxIngestMsg) {
    if let Some(tx_ingest) = TX_INGEST.get() {
        tx_ingest.sender.try_send(msg).ok();
    }
}

const MAX_BUFFERED_TXINGEST_MESSAGES: usize = 10000;

struct TxIngest {
    pub sender: Sender<TxIngestMsg>,

    pub receiver: Receiver<TxIngestMsg>,

    pub default_options: bincode::config::DefaultOptions,
}

static TX_INGEST: OnceLock<TxIngest> = OnceLock::new();
