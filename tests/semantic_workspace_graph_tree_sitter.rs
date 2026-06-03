use pi::semantic_workspace_graph::{
    SemanticEdgeType, SemanticNodeType, SemanticWorkspaceGraphBuilder,
};
use serde_json::Value;

#[test]
fn tree_sitter_rust_symbols_and_calls_are_indexed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_dir = dir.path().join("src");
    let tests_dir = dir.path().join("tests");
    std::fs::create_dir_all(&src_dir).expect("create src dir");
    std::fs::create_dir_all(&tests_dir).expect("create tests dir");
    std::fs::write(
        src_dir.join("lib.rs"),
        r"
            struct Agent;

            fn helper() {}
        ",
    )
    .expect("write rust source");
    std::fs::write(
        tests_dir.join("smoke.rs"),
        r"
            fn helper() {}

            struct Agent;

            #[test]
            fn smoke() {
                helper();
                Agent::new();
                value.render();
            }
        ",
    )
    .expect("write rust test");

    let graph = SemanticWorkspaceGraphBuilder::new(dir.path())
        .build()
        .expect("build semantic graph");

    assert!(graph.nodes.iter().any(|node| {
        node.node_type == SemanticNodeType::CodeSymbol
            && node.title == "Agent"
            && node
                .metadata
                .get("symbol_kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == "struct")
    }));
    assert!(
        graph
            .nodes
            .iter()
            .any(|node| { node.node_type == SemanticNodeType::TestCase && node.title == "smoke" })
    );
    assert_call_edge(&graph, "helper");
    assert_call_edge(&graph, "new");
    assert_call_edge(&graph, "render");
}

fn assert_call_edge(graph: &pi::semantic_workspace_graph::SemanticWorkspaceGraph, callee: &str) {
    assert!(
        graph.edges.iter().any(|edge| {
            edge.edge_type == SemanticEdgeType::Calls
                && edge.reason == "tree_sitter_rust_call"
                && edge
                    .metadata
                    .get("callee")
                    .and_then(Value::as_str)
                    .is_some_and(|actual| actual == callee)
        }),
        "missing call edge for {callee}"
    );
}
