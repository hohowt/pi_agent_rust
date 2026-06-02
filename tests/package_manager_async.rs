use pi::package_manager::{PackageManager, PackageScope, ResolveRoots};
use std::path::Path;

fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(future)
}

fn write_json(path: &Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).expect("serialize json"),
    )
    .expect("write json");
}

#[test]
fn installed_path_resolves_sources_without_external_commands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    let manager = PackageManager::new(cwd.clone());

    let npm_project = run_async(manager.installed_path("npm:react@18.2.0", PackageScope::Project))
        .expect("npm installed_path")
        .expect("npm returns path");
    assert_eq!(
        npm_project,
        cwd.join(".pi")
            .join("npm")
            .join("node_modules")
            .join("react")
    );

    let git_source = "git:https://github.com/example-org/example-repo@main";
    let git_project = run_async(manager.installed_path(git_source, PackageScope::Project))
        .expect("git installed_path")
        .expect("git returns path");
    assert_eq!(
        git_project,
        cwd.join(".pi")
            .join("git")
            .join("github.com")
            .join("example-org")
            .join("example-repo")
    );

    let local_path = run_async(manager.installed_path("./x/../y/thing", PackageScope::Project))
        .expect("local installed_path")
        .expect("local returns path");
    assert_eq!(local_path, cwd.join("y").join("thing"));
}

#[test]
fn resolve_with_roots_merges_project_and_global_local_resources() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path().join("workspace");
    let global_base = temp.path().join("global");
    let project_base = cwd.join(".pi");
    let global_settings_path = global_base.join("settings.json");
    let project_settings_path = cwd.join(".pi").join("settings.json");
    std::fs::create_dir_all(&cwd).expect("create cwd");

    let global_skill = global_base.join("skills").join("review").join("SKILL.md");
    let project_prompt = project_base.join("prompts").join("daily.md");
    if let Some(parent) = global_skill.parent() {
        std::fs::create_dir_all(parent).expect("create global skill dir");
    }
    if let Some(parent) = project_prompt.parent() {
        std::fs::create_dir_all(parent).expect("create project prompt dir");
    }
    std::fs::write(&global_skill, "review skill").expect("write global skill");
    std::fs::write(&project_prompt, "daily prompt").expect("write project prompt");

    write_json(
        &global_settings_path,
        &serde_json::json!({
            "skills": ["skills/review/SKILL.md"]
        }),
    );
    write_json(
        &project_settings_path,
        &serde_json::json!({
            "prompts": ["prompts/daily.md"]
        }),
    );

    let roots = ResolveRoots {
        global_settings_path,
        project_settings_path,
        global_base_dir: global_base,
        project_base_dir: project_base,
        project_settings_enabled: true,
    };
    let resolved =
        run_async(PackageManager::new(cwd).resolve_with_roots(&roots)).expect("resolve roots");

    assert!(resolved.skills.iter().any(|item| item.path == global_skill));
    assert!(
        resolved
            .prompts
            .iter()
            .any(|item| item.path == project_prompt)
    );
}
