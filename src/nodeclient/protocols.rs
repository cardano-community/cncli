use crate::nodeclient::protocols::handshake_protocol::HandshakeProtocol;
use crate::nodeclient::protocols::transaction_protocol::TxSubmissionProtocol;

pub(crate) mod mux_protocol;
mod handshake_protocol;
mod transaction_protocol;
mod chainsync_protocol;

// Who has the ball?
//
// Client agency, we have stuff to send
// Server agency, wait for the server to send us something
#[derive(PartialEq)]
pub enum Agency {
    Client,
    Server,
    None,
}

// Common interface for a protocol
pub trait Protocol {
    // Each protocol has a unique hardcoded id
    fn protocol_id(&self) -> u16;

    // Tells us what agency state the protocol is in
    fn get_agency(&self) -> Agency;

    // Fetch the next piece of data this protocol wants to send, or None if the client doesn't
    // have agency.
    fn send_data(&mut self) -> Option<Vec<u8>>;

    // Process data received from the remote server destined for this protocol
    fn receive_data(&mut self, data: Vec<u8>);
}

pub enum MiniProtocol {
    Handshake(HandshakeProtocol),
    TxSubmission(TxSubmissionProtocol),
    // ...
}

impl Protocol for MiniProtocol {
    fn protocol_id(&self) -> u16 {
        match self {
            MiniProtocol::Handshake(handshake_protocol) => { handshake_protocol.protocol_id() }
            MiniProtocol::TxSubmission(tx_submission_protocol) => { tx_submission_protocol.protocol_id() }
        }
    }

    fn get_agency(&self) -> Agency {
        match self {
            MiniProtocol::Handshake(handshake_protocol) => { handshake_protocol.get_agency() }
            MiniProtocol::TxSubmission(tx_submission_protocol) => { tx_submission_protocol.get_agency() }
        }
    }

    fn send_data(&mut self) -> Option<Vec<u8>> {
        match self {
            MiniProtocol::Handshake(handshake_protocol) => { handshake_protocol.send_data() }
            MiniProtocol::TxSubmission(tx_submission_protocol) => { tx_submission_protocol.send_data() }
        }
    }

    fn receive_data(&mut self, data: Vec<u8>) {
        match self {
            MiniProtocol::Handshake(handshake_protocol) => { handshake_protocol.receive_data(data) }
            MiniProtocol::TxSubmission(tx_submission_protocol) => { tx_submission_protocol.receive_data(data) }
        }
    }
}