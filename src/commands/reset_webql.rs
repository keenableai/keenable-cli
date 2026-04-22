use super::mcp_common;

pub fn reset_webql(selected_flags: Vec<String>) {
    mcp_common::reset(&mcp_common::webql_product(), selected_flags);
}
