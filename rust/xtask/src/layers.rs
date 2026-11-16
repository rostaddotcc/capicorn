//! Layer enforcement: a crate may depend only on LOWER layers; nothing below
//! L6 may depend on a UI toolkit (egui/eframe/winit/rfd and friends).

use std::collections::HashMap;

use cargo_metadata::MetadataCommand;

/// crate dir name -> layer. Apps and xtask sit above everything (L7).
const LAYERS: &[(&str, u8)] = &[
    ("snagcraft-geom", 0),
    ("snagcraft-color", 0),
    ("snagcraft-raster", 0),
    ("snagcraft-doc", 1),
    ("snagcraft-ops", 2),
    ("snagcraft-algo", 2),
    ("snagcraft-text", 2),
    ("snagcraft-vector", 2),
    ("snagcraft-compose", 3),
    ("snagcraft-format", 3),
    ("snagcraft-io", 4),
    ("snagcraft-capture", 4),
    ("snagcraft-engine", 5),
    ("snagcraft-ui-egui", 6),
    ("snagcraft-automation", 6),
    ("snagcraft", 7),
    ("snagcraft-cli", 7),
    ("snagcraft-web", 7),
    ("xtask", 7),
    // W0 probes are L6-equivalent (egui shells).
    ("editor-spike", 6),
    ("viewport-probe", 6),
];

/// UI toolkit crates banned below L6 (checked by name prefix/exact match).
const UI_BANNED: &[&str] = &[
    "egui",
    "eframe",
    "winit",
    "rfd",
    "egui-winit",
    "egui_glow",
    "egui-wgpu",
    "glutin",
    "glutin-winit",
    "accesskit",
    "tray-icon",
    "ksni",
];

fn is_ui_crate(name: &str) -> bool {
    UI_BANNED.iter().any(|b| {
        name == *b || name.starts_with(&format!("{b}-")) || name.starts_with(&format!("{b}_"))
    })
}

pub fn check() -> Result<(), String> {
    let metadata = MetadataCommand::new()
        .manifest_path("./Cargo.toml")
        .exec()
        .map_err(|e| format!("cargo metadata: {e}"))?;

    let layer_of: HashMap<&str, u8> = LAYERS.iter().copied().collect();
    let ws_members: Vec<&str> = metadata
        .workspace_members
        .iter()
        .filter_map(|id| {
            metadata
                .packages
                .iter()
                .find(|p| &p.id == id)
                .map(|p| p.name.as_str())
        })
        .collect();

    let mut errors: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for pkg in metadata
        .packages
        .iter()
        .filter(|p| ws_members.contains(&p.name.as_str()))
    {
        let Some(&layer) = layer_of.get(pkg.name.as_str()) else {
            errors.push(format!(
                "{}: not registered in xtask layers table",
                pkg.name
            ));
            continue;
        };
        for dep in &pkg.dependencies {
            // Only workspace-internal edges are layer-checked; external crates
            // are checked for the UI ban only. Same-layer edges are allowed
            // (the L0 foundation is a DAG); cycles are rejected below.
            if let Some(&dep_layer) = layer_of.get(dep.name.as_str())
                && dep_layer > layer
            {
                errors.push(format!(
                    "{} (L{layer}) depends on {} (L{dep_layer}) — only lower or same layers allowed",
                    pkg.name, dep.name
                ));
            }
            if layer < 6 && is_ui_crate(&dep.name) {
                errors.push(format!(
                    "{} (L{layer}) depends on UI toolkit {} — forbidden below L6",
                    pkg.name, dep.name
                ));
            }
        }
        checked += 1;
    }

    // Cycle detection over workspace-internal edges (same-layer edges allowed,
    // cycles never are).
    let edges: HashMap<&str, Vec<&str>> = metadata
        .packages
        .iter()
        .filter(|p| ws_members.contains(&p.name.as_str()))
        .map(|p| {
            (
                p.name.as_str(),
                p.dependencies
                    .iter()
                    .map(|d| d.name.as_str())
                    .filter(|n| layer_of.contains_key(n))
                    .collect(),
            )
        })
        .collect();
    let mut state: HashMap<&str, u8> = HashMap::new(); // 0=unseen 1=visiting 2=done
    fn visit<'a>(
        node: &'a str,
        edges: &HashMap<&'a str, Vec<&'a str>>,
        state: &mut HashMap<&'a str, u8>,
        path: &mut Vec<&'a str>,
        errors: &mut Vec<String>,
    ) {
        match state.get(node).copied() {
            Some(1) => {
                errors.push(format!("dependency cycle: {} -> {node}", path.join(" -> ")));
                return;
            }
            Some(2) => return,
            _ => {}
        }
        state.insert(node, 1);
        path.push(node);
        if let Some(deps) = edges.get(node) {
            for d in deps {
                visit(d, edges, state, path, errors);
            }
        }
        path.pop();
        state.insert(node, 2);
    }
    for name in edges.keys() {
        visit(name, &edges, &mut state, &mut Vec::new(), &mut errors);
    }

    if errors.is_empty() {
        println!("layers: {checked} workspace crates checked, all edges legal");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("layers: {e}");
        }
        Err(format!("{} layer violation(s)", errors.len()))
    }
}
