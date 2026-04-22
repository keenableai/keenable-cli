use super::mcp_common::{configure, webql_product};

pub async fn configure_webql(selected_flags: Vec<String>) {
    configure(&webql_product(), selected_flags).await;
}
