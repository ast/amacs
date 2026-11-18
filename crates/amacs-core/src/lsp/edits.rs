//! Applying the edits that a server asks for.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;

use amacs_lsp::{Encoding, uri_to_path};
use amacs_text::Text;
use serde_json::Value as Json;

use super::lsp_offset;
use crate::buffer::BufferId;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};

/// The chars that a protocol range covers.
pub(crate) fn char_range(text: &Text, range: &Json, encoding: Encoding) -> Range<usize> {
    let position = |position: &Json| {
        let line = position["line"].as_u64().unwrap_or(0) as usize;
        let units = position["character"].as_u64().unwrap_or(0) as usize;
        lsp_offset(text, line, units, encoding)
    };
    let start = position(&range["start"]);
    start..position(&range["end"]).max(start)
}

impl Editor {
    /// Applies text edits to a buffer. Their ranges are all in terms of the
    /// text as it is before any of them is applied.
    pub(crate) fn lsp_apply_text_edits(
        &mut self,
        id: BufferId,
        edits: &[Json],
        encoding: Encoding,
    ) -> CmdResult {
        let buffer = &mut self.buffers[id];
        let mut resolved: Vec<(Range<usize>, &str)> = edits
            .iter()
            .map(|edit| {
                // A completion's edit may offer two ranges; `insert` leaves
                // the text after point alone.
                let range = edit.get("range").unwrap_or(&edit["insert"]);
                let text = edit["newText"].as_str().unwrap_or("");
                (char_range(buffer.text(), range, encoding), text)
            })
            .collect();
        // From the end backward, so that earlier ranges stay valid. Edits
        // at one position keep their order, which a stable sort and
        // inserting each before the last one gives.
        resolved.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let mut index = 0;
        while index < resolved.len() {
            let start = resolved[index].0.start;
            let group_end = resolved[index..]
                .iter()
                .position(|(range, _)| range.start != start)
                .map_or(resolved.len(), |offset| index + offset);
            for (range, text) in resolved[index..group_end].iter().rev() {
                buffer.delete(range.clone())?;
                buffer.insert(range.start, text)?;
            }
            index = group_end;
        }
        Ok(())
    }

    /// Applies a workspace edit, opening the files it touches that are not
    /// open yet. Returns the number of buffers changed.
    pub(crate) fn lsp_apply_workspace_edit(
        &mut self,
        edit: &Json,
        encoding: Encoding,
    ) -> Result<usize, String> {
        let mut by_file: BTreeMap<PathBuf, Vec<Json>> = BTreeMap::new();
        let mut add = |uri: &Json, edits: &Json| -> Result<(), String> {
            let path = uri
                .as_str()
                .and_then(uri_to_path)
                .ok_or_else(|| format!("cannot edit {uri}: not a file"))?;
            let edits = edits.as_array().cloned().unwrap_or_default();
            by_file.entry(path).or_default().extend(edits);
            Ok(())
        };
        if let Some(changes) = edit["documentChanges"].as_array() {
            for change in changes {
                if let Some(kind) = change["kind"].as_str() {
                    return Err(format!(
                        "the edit wants to {kind} a file, which is not supported"
                    ));
                }
                add(&change["textDocument"]["uri"], &change["edits"])?;
            }
        } else if let Some(changes) = edit["changes"].as_object() {
            for (uri, edits) in changes {
                add(&Json::String(uri.clone()), edits)?;
            }
        }

        let mut changed = 0;
        for (path, edits) in by_file {
            if edits.is_empty() {
                continue;
            }
            let id = self
                .visit_file(&path, false)
                .map_err(|error| error.to_string())?
                .0;
            self.lsp_apply_text_edits(id, &edits, encoding)
                .map_err(|error: CmdError| error.to_string())?;
            changed += 1;
        }
        Ok(changed)
    }
}
