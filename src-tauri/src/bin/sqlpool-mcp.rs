// The read-only MCP server for Claude Desktop. The logic lives in src/mcp.rs.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(sqlpool_lib::mcp_main(&args));
}
