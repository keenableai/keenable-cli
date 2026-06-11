use super::mcp_common;

pub fn reset_webql(selected_flags: Vec<String>, yes: bool) {
    mcp_common::reset(&mcp_common::webql_product(), selected_flags, yes);
}
