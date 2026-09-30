# MySQL comment envelope

This small project-owned grammar recognizes MySQL hash comments while protecting quoted strings, quoted identifiers, dash comments and block comments. Non-comment spans are injected into the existing pinned tree-sitter-sequel SQL highlighter. Execution and completion continue to use the driver-specific SQL parser.

Generated with tree-sitter CLI 0.25.10: `tree-sitter generate --abi 14`. The generated C parser and required header are checked in; normal Cargo builds do not require Node, npm or tree-sitter CLI. Compiler headers are distributed under LICENSE.tree-sitter. No global Cargo registry source is modified.
