use super::mcp_common;

pub fn reset(selected_flags: Vec<String>) {
    mcp_common::reset(&mcp_common::keenable_product(), selected_flags);
}
