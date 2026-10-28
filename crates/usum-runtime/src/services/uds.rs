//! `nwm::UDS`: local wireless (3dbrew "NWM Services"). No other console
//! is ever in range: the game can host a network, which no one joins, and
//! scans find nothing. Commands are added when the game first sends them.

/// The local wireless state of the one client: the event signalled when
/// the connection status changes, and that status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uds {
    pub event: u32,
    pub status: Status,
}

/// 3dbrew "NWMUDS:GetConnectionStatus": the 0x30-byte ConnectionStatus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    pub status: u32,
    pub node_id: u16,
    pub changed_nodes: u16,
    pub nodes: [u16; 16],
    pub total_nodes: u8,
    pub max_nodes: u8,
    pub node_bitmask: u16,
}

/// "Not connected to any network as a host/client, or just initialized."
pub const STATUS_IDLE: u32 = 0x3;
/// "Connected to a network as a host."
pub const STATUS_HOST: u32 = 0x6;

/// NetworkStruct offset of the maximum node count, host included.
pub const NETWORK_MAX_NODES: usize = 0x1D;

impl Status {
    pub fn idle() -> Status {
        Status { status: STATUS_IDLE, ..Status::default() }
    }

    /// Hosting alone: the host is node 1 (bit 0 of the bitmask), and it
    /// is the node that changed.
    pub fn host(max_nodes: u8) -> Status {
        let mut nodes = [0; 16];
        nodes[0] = 1;
        Status { status: STATUS_HOST, node_id: 1, changed_nodes: 1, nodes, total_nodes: 1, max_nodes, node_bitmask: 1 }
    }

    /// The structure as 12 little-endian words.
    pub fn words(&self) -> [u32; 12] {
        let mut b = [0u8; 0x30];
        b[0..4].copy_from_slice(&self.status.to_le_bytes());
        b[8..10].copy_from_slice(&self.node_id.to_le_bytes());
        b[0xA..0xC].copy_from_slice(&self.changed_nodes.to_le_bytes());
        for (i, n) in self.nodes.iter().enumerate() {
            b[0xC + 2 * i..0xE + 2 * i].copy_from_slice(&n.to_le_bytes());
        }
        b[0x2C] = self.total_nodes;
        b[0x2D] = self.max_nodes;
        b[0x2E..0x30].copy_from_slice(&self.node_bitmask.to_le_bytes());
        std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_status_layout() {
        let w = Status::host(8).words();
        assert_eq!(w[0], STATUS_HOST);
        assert_eq!(w[2], 1 | 1 << 16, "node ID 1, changed node 1");
        assert_eq!(w[3] & 0xFFFF, 1, "nodes[0]");
        assert_eq!(w[11], 1 | 8 << 8 | 1 << 16, "total 1, max 8, bitmask 1");
        assert_eq!(Status::idle().words()[0], STATUS_IDLE);
    }
}
