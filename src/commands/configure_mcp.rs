use super::mcp_common::{configure, keenable_product};

pub async fn configure_mcp(selected_flags: Vec<String>, yes: bool) {
    configure(&keenable_product(), selected_flags, yes).await;
}
