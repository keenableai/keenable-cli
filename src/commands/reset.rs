use super::mcp_common;

pub fn reset(selected_flags: Vec<String>, yes: bool) {
    mcp_common::reset(&mcp_common::keenable_product(), selected_flags, yes);
}
