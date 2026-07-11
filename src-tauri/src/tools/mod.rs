pub mod codegraph;
pub mod command;
pub mod execution;
pub mod patch;
pub mod schema;

// Re-exports used by crate-level code
pub(crate) use execution::{
    execute_code_tool_call, tool_call_trace_step, tool_result_trace_step, validate_workspace,
};
pub(crate) use schema::{code_tools_schema, orchestration_tools_schema};

// Re-exports needed by test code in lib.rs
#[cfg(test)]
pub(crate) use codegraph::{
    format_codegraph_explore_output, is_codegraph_status_query, normalize_codegraph_max_files,
    DEFAULT_CODEGRAPH_MAX_FILES, MAX_CODEGRAPH_MAX_FILES,
};
#[cfg(test)]
pub(crate) use execution::{
    delete_workspace_path_tool, move_workspace_path_tool, read_workspace_file_tool,
    resolve_workspace_relative_path, write_workspace_file_tool,
};
