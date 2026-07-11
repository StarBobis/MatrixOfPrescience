use serde_json::{json, Value};

pub(crate) const CODEGRAPH_READ_COMMANDS: &[&str] = &[
    "status", "query", "node", "files", "callers", "callees", "impact", "affected",
];
pub(crate) const CODEGRAPH_WRITE_COMMANDS: &[&str] = &["init", "index", "sync", "unlock"];

fn finalize_tool_function(mut function: Value, strict: bool) -> Value {
    if strict {
        function["strict"] = json!(true);
        function["parameters"]["additionalProperties"] = json!(false);
    }

    json!({
        "type": "function",
        "function": function
    })
}

fn codegraph_command_tool_schema(strict: bool, allow_writes: bool) -> Value {
    let mut commands = CODEGRAPH_READ_COMMANDS.to_vec();
    if allow_writes {
        commands.extend_from_slice(CODEGRAPH_WRITE_COMMANDS);
    }

    finalize_tool_function(
        json!({
            "name": "codegraph_command",
            "description": "Run a supported CodeGraph CLI command in the current workspace. Use status/query/node/files/callers/callees/impact/affected for focused graph reads. Use init/index/sync/unlock to create, rebuild, refresh, or repair the index when write permission allows it. Destructive uninit and global install/upgrade/daemon commands are not available.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": commands,
                        "description": "CodeGraph subcommand to run."
                    },
                    "query": {
                        "type": "string",
                        "description": "Required search text for query."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Required symbol for callers, callees, or impact; optional symbol name for node."
                    },
                    "file": {
                        "type": "string",
                        "description": "Optional indexed file path for node file mode or symbol disambiguation."
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Changed source files used by affected."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum results for query, callers, callees, or node file lines."
                    },
                    "kind": {
                        "type": "string",
                        "description": "Optional symbol kind filter for query, such as function or class."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Dependency traversal depth for impact or affected."
                    },
                    "filter": {
                        "type": "string",
                        "description": "Directory filter for files, or test glob filter for affected."
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Optional glob pattern for files."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["tree", "flat", "grouped"],
                        "description": "Output format for files. Defaults to tree."
                    },
                    "maxDepth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum directory depth for files tree output."
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based starting line for node file mode."
                    },
                    "symbolsOnly": {
                        "type": "boolean",
                        "description": "For node file mode, return only the symbol map and dependents."
                    },
                    "json": {
                        "type": "boolean",
                        "description": "Request JSON output when supported."
                    }
                },
                "required": ["command"]
            }
        }),
        strict,
    )
}

pub(crate) fn code_tools_schema(strict: bool, allow_writes: bool) -> Value {
    let mut tools = vec![
        finalize_tool_function(
            json!({
                "name": "codegraph_explore",
                "description": "Read the current workspace with CodeGraph for symbols, responsibilities, and call paths. The `Found N symbols across M files` line is query-scoped, not the total index file count.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The symbols, files, call flow, or implementation question to inspect."
                        },
                        "maxFiles": {
                            "type": "integer",
                            "description": "Optional maximum number of files to include source from. Defaults to 12 and is capped at 24.",
                            "minimum": 1,
                            "maximum": 24
                        }
                    },
                    "required": ["query"]
                }
            }),
            strict,
        ),
        codegraph_command_tool_schema(strict, allow_writes),
        // read_file, list_files, search_files, glob_files
        finalize_tool_function(
            json!({
                "name": "read_file",
                "description": "Read exact file contents from the workspace with line numbers. Use this when CodeGraph output omits implementation details.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "description": "Workspace-relative file path."
                        },
                        "startLine": {
                            "type": "integer",
                            "description": "1-based line to start from. Defaults to 1.",
                            "minimum": 1
                        },
                        "maxLines": {
                            "type": "integer",
                            "description": "Maximum lines to read. Defaults to 240 and is capped at 1000.",
                            "minimum": 1,
                            "maximum": 1000
                        }
                    },
                    "required": ["file"]
                }
            }),
            strict,
        ),
        finalize_tool_function(
            json!({
                "name": "list_files",
                "description": "List files under the workspace or a subdirectory. Use this to discover nearby files before reading them.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative directory path."
                        },
                        "recursive": {
                            "type": "boolean",
                            "description": "Whether to recurse. Defaults to true."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum files to return. Defaults to 120 and is capped at 500.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    },
                    "required": []
                }
            }),
            strict,
        ),
        finalize_tool_function(
            json!({
                "name": "search_files",
                "description": "Search text in workspace files with ripgrep-style output. Use for finding identifiers, errors, strings, and TODOs.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Regex or literal text to search for."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative path to search in."
                        },
                        "caseSensitive": {
                            "type": "boolean",
                            "description": "Defaults to false."
                        },
                        "literal": {
                            "type": "boolean",
                            "description": "Treat query as fixed text instead of regex. Defaults to false."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum matches to return. Defaults to 80 and is capped at 300.",
                            "minimum": 1,
                            "maximum": 300
                        }
                    },
                    "required": ["query"]
                }
            }),
            strict,
        ),
        finalize_tool_function(
            json!({
                "name": "glob_files",
                "description": "Find files by glob pattern, for example `src/**/*.vue` or `**/*.rs`.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern relative to the workspace."
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional workspace-relative directory to search from."
                        },
                        "maxResults": {
                            "type": "integer",
                            "description": "Maximum files to return. Defaults to 120 and is capped at 500.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    },
                    "required": ["pattern"]
                }
            }),
            strict,
        ),
    ];

    if allow_writes {
        tools.extend([
            finalize_tool_function(
                json!({
                    "name": "write_file",
                    "description": "Create, overwrite, or append to a UTF-8 text file inside the workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file": {
                                "type": "string",
                                "description": "Workspace-relative file path."
                            },
                            "content": {
                                "type": "string",
                                "description": "Text content to write."
                            },
                            "mode": {
                                "type": "string",
                                "enum": ["overwrite", "create", "append"],
                                "description": "Write mode. Defaults to overwrite."
                            },
                            "createParents": {
                                "type": "boolean",
                                "description": "Whether to create missing parent directories. Defaults to true."
                            }
                        },
                        "required": ["file", "content"]
                    }
                }),
                strict
            ),
            finalize_tool_function(
                json!({
                    "name": "create_directory",
                    "description": "Create a directory inside the workspace, including missing parents.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Workspace-relative directory path."
                            }
                        },
                        "required": ["path"]
                    }
                }),
                strict
            ),
            finalize_tool_function(
                json!({
                    "name": "delete_path",
                    "description": "Delete a workspace-relative file or, with recursive=true, a directory. Refuses workspace root and sensitive/generated paths.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Workspace-relative path to delete."
                            },
                            "recursive": {
                                "type": "boolean",
                                "description": "Required for deleting directories. Defaults to false."
                            }
                        },
                        "required": ["path"]
                    }
                }),
                strict
            ),
            finalize_tool_function(
                json!({
                    "name": "move_path",
                    "description": "Move or rename a file or directory inside the workspace.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "from": {
                                "type": "string",
                                "description": "Existing workspace-relative source path."
                            },
                            "to": {
                                "type": "string",
                                "description": "Workspace-relative destination path."
                            },
                            "createParents": {
                                "type": "boolean",
                                "description": "Whether to create missing destination parent directories. Defaults to true."
                            }
                        },
                        "required": ["from", "to"]
                    }
                }),
                strict
            ),
            finalize_tool_function(
                json!({
                    "name": "apply_patch",
                    "description": "Apply a unified diff patch inside the workspace. Keep each call focused on one file and one coherent change; split large edits across multiple calls instead of rewriting a whole file. Before patching, read the exact current target location. Build hunks from stable nearby content, not remembered or hand-guessed line numbers. Every hunk body line must start with ' ', '+', '-', or '\\'. Hunk line counts must match the body. Use checkOnly=true to validate non-trivial patches before applying.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "patchText": {
                                "type": "string",
                                "description": "Unified diff text accepted by git apply. Context lines require a leading marker space before their source indentation. Prefer exact old lines plus a few stable context lines; avoid fragile @@ line ranges unless they were generated from freshly read content."
                            },
                            "checkOnly": {
                                "type": "boolean",
                                "description": "Validate only without applying. Defaults to false."
                            }
                        },
                        "required": ["patchText"]
                    }
                }),
                strict
            ),
            finalize_tool_function(
                json!({
                    "name": "run_command",
                    "description": "Run a non-interactive command in the workspace, such as tests, formatters, or git diff. Prefer command plus args instead of shell syntax.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "string",
                                "description": "Executable name, for example npm, cargo, git, rg, node, or python."
                            },
                            "args": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Command arguments."
                            },
                            "cwd": {
                                "type": "string",
                                "description": "Optional workspace-relative working directory."
                            },
                            "timeoutMs": {
                                "type": "integer",
                                "description": "Timeout in milliseconds. Defaults to 30000 and is capped at 120000.",
                                "minimum": 1000,
                                "maximum": 120000
                            },
                            "env": {
                                "type": "object",
                                "additionalProperties": { "type": "string" },
                                "description": "Optional environment variable overrides for the child process. The command still inherits the Matrix app process environment first."
                            }
                        },
                        "required": ["command"]
                    }
                }),
                strict
            )
        ]);
    }

    json!(tools)
}

pub(crate) fn orchestration_tools_schema(strict: bool) -> Value {
    json!([finalize_tool_function(
        json!({
            "name": "dispatch_tasks",
            "description": "Formally dispatch tasks to specific group members. Only the coordinator should call this tool. Each entry assigns exactly one task to one member. Use this structured tool instead of writing assignment text; the system will route each task to its target member automatically.",
            "parameters": {
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "member": {
                                    "type": "string",
                                    "description": "The exact name of the group member to assign this task to."
                                },
                                "instruction": {
                                    "type": "string",
                                    "description": "The specific task instruction. Include the goal, expected output, and any constraints. Be precise and actionable."
                                }
                            },
                            "required": ["member", "instruction"]
                        },
                        "description": "One or more task assignments. Each entry dispatches a single task to one member."
                    }
                },
                "required": ["tasks"]
            }
        }),
        strict,
    )])
}
