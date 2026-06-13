use super::mcp_common::configure;

pub async fn configure_mcp(selected_flags: Vec<String>, yes: bool) {
    configure(selected_flags, yes).await;
}
