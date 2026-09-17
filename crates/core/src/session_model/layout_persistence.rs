use crate::session_model::layout::*;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub enum PersistedNativeLayoutNode {
    Leaf {
        pane: usize,
    },
    Split {
        axis: PaneResizeAxis,
        ratio: f32,
        first: Box<PersistedNativeLayoutNode>,
        second: Box<PersistedNativeLayoutNode>,
    },
}

impl NativeLayout {
    pub fn persisted_layout_tree_from_native(
        node: &NativePaneLayoutNode,
        pane_indices: &HashMap<String, usize>,
    ) -> Option<PersistedNativeLayoutNode> {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => Some(PersistedNativeLayoutNode::Leaf {
                pane: *pane_indices.get(pane_id)?,
            }),
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => Some(PersistedNativeLayoutNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(Self::persisted_layout_tree_from_native(
                    first,
                    pane_indices,
                )?),
                second: Box::new(Self::persisted_layout_tree_from_native(
                    second,
                    pane_indices,
                )?),
            }),
        }
    }

    pub fn native_layout_tree_from_persisted(
        node: &PersistedNativeLayoutNode,
        pane_ids: &[String],
    ) -> Option<NativePaneLayoutNode> {
        match node {
            PersistedNativeLayoutNode::Leaf { pane } => {
                let pane_id = pane_ids.get(*pane)?.clone();
                Some(NativePaneLayoutNode::Leaf { pane_id })
            }
            PersistedNativeLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => Some(NativePaneLayoutNode::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(Self::native_layout_tree_from_persisted(first, pane_ids)?),
                second: Box::new(Self::native_layout_tree_from_persisted(second, pane_ids)?),
            }),
        }
    }

    pub fn persisted_layout_tree_to_value(node: PersistedNativeLayoutNode) -> Value {
        match node {
            PersistedNativeLayoutNode::Leaf { pane } => json!({
                "kind": "leaf",
                "pane": pane,
            }),
            PersistedNativeLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => json!({
                "kind": "split",
                "axis": match axis {
                    PaneResizeAxis::Horizontal => "horizontal",
                    PaneResizeAxis::Vertical => "vertical",
                },
                "ratio": ratio,
                "first": Self::persisted_layout_tree_to_value(*first),
                "second": Self::persisted_layout_tree_to_value(*second),
            }),
        }
    }

    pub fn parse_persisted_layout_tree_value(
        value: &Value,
    ) -> Result<PersistedNativeLayoutNode, String> {
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "layout tree node is missing 'kind'".to_string())?;
        match kind {
            "leaf" => {
                let pane = value
                    .get("pane")
                    .and_then(Value::as_u64)
                    .and_then(|raw| usize::try_from(raw).ok())
                    .ok_or_else(|| "layout tree leaf is missing valid 'pane'".to_string())?;
                Ok(PersistedNativeLayoutNode::Leaf { pane })
            }
            "split" => {
                let axis = match value
                    .get("axis")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "layout tree split is missing 'axis'".to_string())?
                {
                    "horizontal" => PaneResizeAxis::Horizontal,
                    "vertical" => PaneResizeAxis::Vertical,
                    other => {
                        return Err(format!("layout tree split axis '{other}' is invalid"));
                    }
                };
                let ratio = value
                    .get("ratio")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| "layout tree split is missing 'ratio'".to_string())?
                    as f32;
                if !ratio.is_finite() {
                    return Err("layout tree split ratio must be finite".to_string());
                }
                Ok(PersistedNativeLayoutNode::Split {
                    axis,
                    ratio,
                    first: Box::new(Self::parse_persisted_layout_tree_value(
                        value
                            .get("first")
                            .ok_or_else(|| "layout tree split is missing 'first'".to_string())?,
                    )?),
                    second: Box::new(Self::parse_persisted_layout_tree_value(
                        value
                            .get("second")
                            .ok_or_else(|| "layout tree split is missing 'second'".to_string())?,
                    )?),
                })
            }
            other => Err(format!("layout tree node kind '{other}' is invalid")),
        }
    }
}
