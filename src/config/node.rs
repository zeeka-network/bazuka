use crate::node::NodeOptions;
use std::time::Duration;

pub fn get_node_options() -> NodeOptions {
    NodeOptions {
        heartbeat_interval: Duration::from_secs(1),
        num_peers: 8,
        no_response_punish: 5,
        invalid_data_punish: 10,
        incorrect_power_punish: 12,
        max_punish: 15,
        outdated_heights_threshold: 10,
        state_unavailable_ban_time: 20,
    }
}

pub fn get_test_node_options() -> NodeOptions {
    NodeOptions {
        heartbeat_interval: Duration::from_millis(300),
        num_peers: 8,
        no_response_punish: 0,
        invalid_data_punish: 0,
        incorrect_power_punish: 0,
        max_punish: 0,
        outdated_heights_threshold: 5,
        state_unavailable_ban_time: 10,
    }
}
