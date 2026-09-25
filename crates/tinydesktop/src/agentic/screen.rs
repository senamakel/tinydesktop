//! Accessibility snapshot parsing and compact Jev candidate descriptions.

use serde::Deserialize;
use serde_json::{Value, json};
use tinydesktop_bus::{DesktopResponse, SnapshotRequest, Surface};

use crate::Desktop;

const MAX_TREE_DEPTH: usize = 64;
const MAX_VISITED_NODES: usize = 4_096;

/// One ref-bearing accessibility node offered to Jev.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct Candidate {
    pub(super) ref_id: String,
    pub(super) role: String,
    pub(super) name: Option<String>,
    pub(super) description: Option<String>,
    pub(super) native_id: Option<NativeId>,
    pub(super) value: Option<Value>,
    pub(super) states: Vec<String>,
    pub(super) available_actions: Vec<String>,
    pub(super) children_count: Option<usize>,
    pub(super) bounds: Option<Value>,
    pub(super) children: Vec<Candidate>,
    #[serde(skip)]
    pub(super) path: Vec<String>,
}

/// Engine-provided platform identifier for an otherwise unlabeled element.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(super) struct NativeId {
    pub(super) kind: String,
    pub(super) value: String,
}

impl Candidate {
    pub(super) fn labels(&self) -> impl Iterator<Item = &str> {
        [
            self.name.as_deref(),
            self.description.as_deref(),
            self.native_id.as_ref().map(|id| id.value.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter(|label| !label.is_empty())
    }

    pub(super) fn label(&self) -> Option<&str> {
        self.labels().next()
    }
}

/// Parsed current surface.
#[derive(Debug, Clone)]
pub(super) struct Screen {
    pub(super) app: String,
    pub(super) window: Option<String>,
    pub(super) surface: String,
    pub(super) root: Option<String>,
    pub(super) candidates: Vec<Candidate>,
    pub(super) observed: Vec<Candidate>,
}

pub(super) fn observe(
    desktop: &Desktop,
    app: &str,
    root: Option<&str>,
) -> Result<Screen, Box<DesktopResponse>> {
    let request = SnapshotRequest {
        app: Some(app.to_owned()),
        include_bounds: true,
        interactive_only: false,
        compact: true,
        root_ref: root.map(str::to_owned),
        ..SnapshotRequest::default()
    };
    let mut reply = desktop.snapshot(request);
    if !reply.ok && root.is_none() {
        reply = desktop.snapshot(SnapshotRequest {
            app: Some(app.to_owned()),
            max_depth: Some(4),
            include_bounds: true,
            compact: true,
            ..SnapshotRequest::default()
        });
    }
    parse_reply(desktop, app, root, reply)
}

pub(super) fn parse_reply(
    desktop: &Desktop,
    app: &str,
    root: Option<&str>,
    mut reply: DesktopResponse,
) -> Result<Screen, Box<DesktopResponse>> {
    if !reply.ok {
        return Err(Box::new(reply));
    }
    let mut surface = "window".to_owned();
    if root.is_none()
        && let Some(role) = overlay_role(reply.data.as_ref().and_then(|data| data.get("tree")))
    {
        let overlay = match role {
            "sheet" => Surface::Sheet,
            "alert" => Surface::Alert,
            "menu" => Surface::Menu,
            "popover" => Surface::Popover,
            _ => Surface::Window,
        };
        let scoped = desktop.snapshot(SnapshotRequest {
            app: Some(app.to_owned()),
            include_bounds: true,
            compact: true,
            surface: overlay,
            ..SnapshotRequest::default()
        });
        if scoped.ok {
            reply = scoped;
            role.clone_into(&mut surface);
        }
    }
    let Some(data) = reply.data.as_ref() else {
        return Err(Box::new(DesktopResponse::err(
            "snapshot",
            tinydesktop_bus::DesktopError::new("INTERNAL", "successful snapshot carried no data"),
        )));
    };
    let tree = data.get("tree").cloned().unwrap_or(Value::Null);
    let mut root_node: Candidate = serde_json::from_value(tree).unwrap_or_default();
    let mut candidates = Vec::new();
    let mut visited = 0_usize;
    collect(&mut root_node, &[], &mut candidates, 0, &mut visited);
    let observed = candidates.clone();
    candidates.retain(offerable);
    candidates.truncate(254);

    Ok(Screen {
        app: data
            .get("app")
            .and_then(Value::as_str)
            .unwrap_or(app)
            .to_owned(),
        window: data
            .get("window")
            .and_then(|window| window.get("title"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        surface,
        root: root.map(str::to_owned),
        candidates,
        observed,
    })
}

fn collect(
    node: &mut Candidate,
    path: &[String],
    out: &mut Vec<Candidate>,
    depth: usize,
    visited: &mut usize,
) {
    if depth > MAX_TREE_DEPTH || *visited >= MAX_VISITED_NODES {
        return;
    }
    *visited = visited.saturating_add(1);
    let label = node.label().map_or_else(
        || node.role.clone(),
        |name| format!("{} {name:?}", node.role),
    );
    node.path = path.to_vec();
    if !node.ref_id.is_empty() {
        out.push(node.clone());
    }
    let mut child_path = path.to_vec();
    if !node.children.is_empty() {
        child_path.push(label);
    }
    for child in &mut node.children {
        collect(child, &child_path, out, depth.saturating_add(1), visited);
    }
}

fn offerable(node: &Candidate) -> bool {
    !node.available_actions.is_empty()
        && !node
            .states
            .iter()
            .any(|state| matches!(state.as_str(), "disabled" | "hidden"))
}

fn overlay_role(tree: Option<&Value>) -> Option<&'static str> {
    let mut pending = vec![(tree?, 0_usize)];
    let mut visited = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_TREE_DEPTH || visited >= MAX_VISITED_NODES {
            continue;
        }
        visited = visited.saturating_add(1);
        if let Some(role) = node.get("role").and_then(Value::as_str)
            && matches!(role, "sheet" | "alert" | "menu" | "popover")
        {
            return Some(match role {
                "sheet" => "sheet",
                "alert" => "alert",
                "menu" => "menu",
                _ => "popover",
            });
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            pending.extend(
                children
                    .iter()
                    .rev()
                    .map(|child| (child, depth.saturating_add(1))),
            );
        }
    }
    None
}

pub(super) fn describe(node: &Candidate, include_values: bool) -> Value {
    let mut value = json!({
        "what": format!(
            "{}{}",
            node.role,
            node.label()
                .map_or_else(String::new, |name| format!(" {name:?}"))
        ),
        "where": if node.path.is_empty() { "top level".to_owned() } else { node.path.join(" > ") },
        "supports": node.available_actions,
    });
    if include_values && let Some(held) = node.value.as_ref().filter(|value| !value.is_null()) {
        value["holds"] = Value::String(held.to_string().chars().take(120).collect());
    }
    if !node.states.is_empty() {
        value["state"] = Value::String(node.states.join(", "));
    }
    if let Some(count) = node.children_count {
        value["contains"] = json!(count);
    }
    if node.label().is_none()
        && node.value.is_none()
        && let Some(bounds) = &node.bounds
    {
        value["bounds"] = bounds.clone();
    }
    json!({"untrusted_accessibility_data": value})
}

pub(super) fn fingerprint(screen: &Screen) -> String {
    screen
        .candidates
        .iter()
        .map(|node| {
            format!(
                "{}:{}:{}:{:?}",
                node.ref_id,
                node.role,
                node.label().unwrap_or_default(),
                (node.states.as_slice(), node.value.as_ref())
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}
