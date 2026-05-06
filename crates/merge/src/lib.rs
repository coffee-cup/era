//! Snapshot-agnostic file merge engine.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Snapshot-agnostic file versions supplied to a merge strategy.
#[derive(Debug, Clone, Copy)]
pub struct FileMergeInput<'a> {
    path: Option<&'a Path>,
    base: Option<&'a [u8]>,
    ours: Option<&'a [u8]>,
    theirs: Option<&'a [u8]>,
}

impl<'a> FileMergeInput<'a> {
    /// Creates merge input for one path.
    #[must_use]
    pub fn new(
        path: Option<&'a Path>,
        base: Option<&'a [u8]>,
        ours: Option<&'a [u8]>,
        theirs: Option<&'a [u8]>,
    ) -> Self {
        Self {
            path,
            base,
            ours,
            theirs,
        }
    }

    /// Optional path hint used by strategy selection and conflict reporting.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path
    }

    /// Common ancestor bytes, or absent when both sides added independently.
    #[must_use]
    pub fn base(&self) -> Option<&[u8]> {
        self.base
    }

    /// Our version bytes, or absent when we deleted the file.
    #[must_use]
    pub fn ours(&self) -> Option<&[u8]> {
        self.ours
    }

    /// Their version bytes, or absent when they deleted the file.
    #[must_use]
    pub fn theirs(&self) -> Option<&[u8]> {
        self.theirs
    }
}

/// Runs merge strategies in order and falls back when a strategy does not support an input.
pub struct MergeEngine {
    strategies: Vec<Box<dyn FileMergeStrategy>>,
}

impl MergeEngine {
    /// Creates an engine with no strategies.
    #[must_use]
    pub fn new() -> Self {
        Self {
            strategies: Vec::new(),
        }
    }

    /// Creates the default file merge engine.
    #[must_use]
    pub fn default_text() -> Self {
        Self::new().with_strategy(LineMergeStrategy::new())
    }

    /// Appends a strategy to the fallback chain.
    #[must_use]
    pub fn with_strategy(mut self, strategy: impl FileMergeStrategy + 'static) -> Self {
        self.strategies.push(Box::new(strategy));
        self
    }

    /// Merges a file with the first strategy that supports the input.
    #[must_use]
    pub fn merge(&self, input: &FileMergeInput<'_>) -> MergeResult {
        for strategy in &self.strategies {
            match strategy.merge(input) {
                StrategyMergeResult::Merged(outcome) => {
                    return MergeResult::new(strategy.name(), outcome);
                }
                StrategyMergeResult::Unsupported(_) => {}
            }
        }

        MergeResult::new(
            "none",
            MergeOutcome::Conflicted {
                hunks: vec![MergeHunk::Conflict(MergeConflict::whole_file(
                    ConflictKind::Unsupported,
                    input,
                ))],
            },
        )
    }
}

impl Default for MergeEngine {
    fn default() -> Self {
        Self::default_text()
    }
}

/// Merges with the default line-oriented strategy.
#[must_use]
pub fn merge_file(input: &FileMergeInput<'_>) -> MergeResult {
    MergeEngine::default_text().merge(input)
}

/// A file merge strategy such as line-based, JavaScript semantic, or JSON semantic merge.
pub trait FileMergeStrategy: Send + Sync {
    /// Stable strategy name recorded on merge results.
    fn name(&self) -> &'static str;

    /// Attempts to merge the input or declines so another strategy can try.
    fn merge(&self, input: &FileMergeInput<'_>) -> StrategyMergeResult;
}

/// Result returned by a strategy before the engine attaches the selected strategy name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyMergeResult {
    /// Strategy handled the input.
    Merged(MergeOutcome),
    /// Strategy declined the input so the engine can try the next strategy.
    Unsupported(UnsupportedMerge),
}

/// Why a strategy declined an input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedMerge {
    reason: String,
}

impl UnsupportedMerge {
    /// Creates an unsupported result with a human-readable reason.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// Returns the reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Merge result with the selected strategy name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeResult {
    strategy: &'static str,
    outcome: MergeOutcome,
}

impl MergeResult {
    /// Creates a merge result.
    #[must_use]
    pub fn new(strategy: &'static str, outcome: MergeOutcome) -> Self {
        Self { strategy, outcome }
    }

    /// Strategy that produced this result.
    #[must_use]
    pub fn strategy(&self) -> &'static str {
        self.strategy
    }

    /// Returns the merge outcome.
    #[must_use]
    pub fn outcome(&self) -> &MergeOutcome {
        &self.outcome
    }

    /// Returns true when the merge produced a complete resolution.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        matches!(self.outcome, MergeOutcome::Resolved(_))
    }

    /// Returns the resolved file when there were no conflicts.
    #[must_use]
    pub fn resolved_file(&self) -> Option<&MergedFile> {
        match &self.outcome {
            MergeOutcome::Resolved(file) => Some(file),
            MergeOutcome::Conflicted { .. } => None,
        }
    }

    /// Returns resolved bytes when the merge produced a present file.
    #[must_use]
    pub fn resolved_bytes(&self) -> Option<&[u8]> {
        match self.resolved_file() {
            Some(MergedFile::Present(bytes)) => Some(bytes.as_slice()),
            Some(MergedFile::Deleted) | None => None,
        }
    }

    /// Returns all structured conflicts in this result.
    #[must_use]
    pub fn conflicts(&self) -> Vec<&MergeConflict> {
        match &self.outcome {
            MergeOutcome::Resolved(_) => Vec::new(),
            MergeOutcome::Conflicted { hunks } => hunks
                .iter()
                .filter_map(|hunk| match hunk {
                    MergeHunk::Resolved(_) => None,
                    MergeHunk::Conflict(conflict) => Some(conflict),
                })
                .collect(),
        }
    }
}

/// Complete merge outcome for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Merge completed without conflicts.
    Resolved(MergedFile),
    /// Merge needs resolution. Resolved hunks preserve deterministic context.
    Conflicted { hunks: Vec<MergeHunk> },
}

/// Resolved file state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergedFile {
    /// File exists with these bytes.
    Present(Vec<u8>),
    /// File should be deleted.
    Deleted,
}

/// A resolved or conflicted part of a text merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeHunk {
    /// Bytes accepted automatically.
    Resolved(Vec<u8>),
    /// Bytes requiring explicit resolution.
    Conflict(MergeConflict),
}

/// Structured conflict details shared by all strategies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    kind: ConflictKind,
    path: Option<PathBuf>,
    base: Option<Vec<u8>>,
    ours: Option<Vec<u8>>,
    theirs: Option<Vec<u8>>,
}

impl MergeConflict {
    fn whole_file(kind: ConflictKind, input: &FileMergeInput<'_>) -> Self {
        Self {
            kind,
            path: input.path().map(Path::to_path_buf),
            base: input.base().map(<[u8]>::to_vec),
            ours: input.ours().map(<[u8]>::to_vec),
            theirs: input.theirs().map(<[u8]>::to_vec),
        }
    }

    /// Creates a conflict value.
    #[must_use]
    pub fn new(
        kind: ConflictKind,
        path: Option<PathBuf>,
        base: Option<Vec<u8>>,
        ours: Option<Vec<u8>>,
        theirs: Option<Vec<u8>>,
    ) -> Self {
        Self {
            kind,
            path,
            base,
            ours,
            theirs,
        }
    }

    /// Conflict kind.
    #[must_use]
    pub fn kind(&self) -> ConflictKind {
        self.kind
    }

    /// Path hint associated with this conflict.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Base bytes for this conflict, if any.
    #[must_use]
    pub fn base(&self) -> Option<&[u8]> {
        self.base.as_deref()
    }

    /// Our bytes for this conflict, if any.
    #[must_use]
    pub fn ours(&self) -> Option<&[u8]> {
        self.ours.as_deref()
    }

    /// Their bytes for this conflict, if any.
    #[must_use]
    pub fn theirs(&self) -> Option<&[u8]> {
        self.theirs.as_deref()
    }
}

/// Structured conflict kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    /// Both sides added a file with different contents.
    BothAdded,
    /// Both sides inserted different content at the same position.
    BothInserted,
    /// Both sides changed overlapping content differently.
    BothModified,
    /// One side deleted content changed by the other side.
    ModifyDelete,
    /// File is not safe for line-oriented text merge.
    Binary,
    /// File exceeds the in-memory text merge cutoff.
    TooLarge,
    /// No configured strategy supported the input.
    Unsupported,
}

/// Deterministic line-oriented three-way merge strategy.
#[derive(Debug, Clone, Copy, Default)]
pub struct LineMergeStrategy;

impl LineMergeStrategy {
    /// Creates a line merge strategy.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Merges with the line-oriented strategy.
    #[must_use]
    pub fn merge_file(&self, input: &FileMergeInput<'_>) -> MergeOutcome {
        merge_line_input(input)
    }
}

const MAX_TEXT_MERGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_LCS_CELLS: usize = 4_000_000;

type Line = Vec<u8>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditRegion {
    start: usize,
    end: usize,
    replacement: Vec<Line>,
}

impl EditRegion {
    fn new(start: usize, end: usize, replacement: Vec<Line>) -> Self {
        Self {
            start,
            end,
            replacement,
        }
    }

    fn is_insertion(&self) -> bool {
        self.start == self.end
    }
}

fn merge_line_input(input: &FileMergeInput<'_>) -> MergeOutcome {
    match (input.base(), input.ours(), input.theirs()) {
        (None, None, None) => resolved_deleted(),
        (_, None, None) => resolved_deleted(),
        (_, Some(ours), Some(theirs)) if ours == theirs => resolved_present(ours),
        (Some(base), Some(ours), theirs) if base == ours => match theirs {
            Some(theirs) => resolved_present(theirs),
            None => resolved_deleted(),
        },
        (Some(base), ours, Some(theirs)) if base == theirs => match ours {
            Some(ours) => resolved_present(ours),
            None => resolved_deleted(),
        },
        (None, Some(ours), None) => resolved_present(ours),
        (None, None, Some(theirs)) => resolved_present(theirs),
        (None, Some(_), Some(_)) => whole_file_conflict(ConflictKind::BothAdded, input),
        (Some(base), None, Some(theirs)) => {
            if base == theirs {
                resolved_deleted()
            } else {
                whole_file_conflict(ConflictKind::ModifyDelete, input)
            }
        }
        (Some(base), Some(ours), None) => {
            if base == ours {
                resolved_deleted()
            } else {
                whole_file_conflict(ConflictKind::ModifyDelete, input)
            }
        }
        (Some(base), Some(ours), Some(theirs)) => merge_present_lines(input, base, ours, theirs),
    }
}

fn resolved_present(bytes: &[u8]) -> MergeOutcome {
    MergeOutcome::Resolved(MergedFile::Present(bytes.to_vec()))
}

fn resolved_deleted() -> MergeOutcome {
    MergeOutcome::Resolved(MergedFile::Deleted)
}

fn whole_file_conflict(kind: ConflictKind, input: &FileMergeInput<'_>) -> MergeOutcome {
    MergeOutcome::Conflicted {
        hunks: vec![MergeHunk::Conflict(MergeConflict::whole_file(kind, input))],
    }
}

fn merge_present_lines(
    input: &FileMergeInput<'_>,
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
) -> MergeOutcome {
    if [base, ours, theirs].iter().any(|bytes| bytes.contains(&0)) {
        return whole_file_conflict(ConflictKind::Binary, input);
    }
    if [base, ours, theirs]
        .iter()
        .any(|bytes| bytes.len() > MAX_TEXT_MERGE_BYTES)
    {
        return whole_file_conflict(ConflictKind::TooLarge, input);
    }

    let base_lines = split_lines(base);
    let ours_lines = split_lines(ours);
    let theirs_lines = split_lines(theirs);
    let ours_edits = edit_regions(&base_lines, &ours_lines);
    let theirs_edits = edit_regions(&base_lines, &theirs_lines);
    let mut hunks = Vec::new();
    let mut base_pos = 0;
    let mut ours_index = 0;
    let mut theirs_index = 0;
    let mut conflicted = false;
    let path = input.path().map(Path::to_path_buf);

    while ours_index < ours_edits.len() || theirs_index < theirs_edits.len() {
        match (ours_edits.get(ours_index), theirs_edits.get(theirs_index)) {
            (Some(ours_edit), Some(theirs_edit))
                if ours_edit.is_insertion()
                    && theirs_edit.is_insertion()
                    && ours_edit.start == theirs_edit.start =>
            {
                push_resolved_lines(&mut hunks, &base_lines[base_pos..ours_edit.start]);
                base_pos = ours_edit.start;
                if ours_edit.replacement == theirs_edit.replacement {
                    push_resolved_lines(&mut hunks, &ours_edit.replacement);
                } else {
                    conflicted = true;
                    push_conflict(
                        &mut hunks,
                        ConflictKind::BothInserted,
                        path.clone(),
                        Some(Vec::new()),
                        Some(lines_to_bytes(&ours_edit.replacement)),
                        Some(lines_to_bytes(&theirs_edit.replacement)),
                    );
                }
                ours_index += 1;
                theirs_index += 1;
            }
            (Some(ours_edit), Some(theirs_edit)) if ours_edit.end <= theirs_edit.start => {
                apply_resolved_edit(&mut hunks, &base_lines, &mut base_pos, ours_edit);
                ours_index += 1;
            }
            (Some(ours_edit), Some(theirs_edit)) if theirs_edit.end <= ours_edit.start => {
                apply_resolved_edit(&mut hunks, &base_lines, &mut base_pos, theirs_edit);
                theirs_index += 1;
            }
            (Some(ours_edit), Some(theirs_edit)) => {
                let conflict_start = ours_edit.start.min(theirs_edit.start);
                let (conflict_end, next_ours, next_theirs) = conflict_group(
                    &ours_edits,
                    ours_index,
                    &theirs_edits,
                    theirs_index,
                    conflict_start,
                    ours_edit.end.max(theirs_edit.end),
                );
                push_resolved_lines(&mut hunks, &base_lines[base_pos..conflict_start]);

                let ours_bytes = apply_edits_to_range(
                    &base_lines,
                    &ours_edits[ours_index..next_ours],
                    conflict_start,
                    conflict_end,
                );
                let theirs_bytes = apply_edits_to_range(
                    &base_lines,
                    &theirs_edits[theirs_index..next_theirs],
                    conflict_start,
                    conflict_end,
                );
                let base_bytes = lines_to_bytes(&base_lines[conflict_start..conflict_end]);

                if ours_bytes == theirs_bytes {
                    push_resolved_bytes(&mut hunks, ours_bytes);
                } else {
                    conflicted = true;
                    let kind = classify_content_conflict(&base_bytes, &ours_bytes, &theirs_bytes);
                    push_conflict(
                        &mut hunks,
                        kind,
                        path.clone(),
                        Some(base_bytes),
                        Some(ours_bytes),
                        Some(theirs_bytes),
                    );
                }

                base_pos = conflict_end;
                ours_index = next_ours;
                theirs_index = next_theirs;
            }
            (Some(ours_edit), None) => {
                apply_resolved_edit(&mut hunks, &base_lines, &mut base_pos, ours_edit);
                ours_index += 1;
            }
            (None, Some(theirs_edit)) => {
                apply_resolved_edit(&mut hunks, &base_lines, &mut base_pos, theirs_edit);
                theirs_index += 1;
            }
            (None, None) => break,
        }
    }

    push_resolved_lines(&mut hunks, &base_lines[base_pos..]);

    if conflicted {
        MergeOutcome::Conflicted { hunks }
    } else {
        MergeOutcome::Resolved(MergedFile::Present(concat_resolved_hunks(&hunks)))
    }
}

fn split_lines(bytes: &[u8]) -> Vec<Line> {
    if bytes.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(bytes[start..=index].to_vec());
            start = index + 1;
        }
    }
    if start < bytes.len() {
        lines.push(bytes[start..].to_vec());
    }
    lines
}

fn edit_regions(base: &[Line], side: &[Line]) -> Vec<EditRegion> {
    let matches = matching_pairs(base, side);
    let mut edits = Vec::new();
    let mut base_cursor = 0;
    let mut side_cursor = 0;

    for (base_index, side_index) in matches {
        if base_cursor != base_index || side_cursor != side_index {
            edits.push(EditRegion::new(
                base_cursor,
                base_index,
                side[side_cursor..side_index].to_vec(),
            ));
        }
        base_cursor = base_index + 1;
        side_cursor = side_index + 1;
    }

    if base_cursor != base.len() || side_cursor != side.len() {
        edits.push(EditRegion::new(
            base_cursor,
            base.len(),
            side[side_cursor..].to_vec(),
        ));
    }

    edits
}

fn matching_pairs(base: &[Line], side: &[Line]) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    collect_patience_matches(base, side, 0, base.len(), 0, side.len(), &mut matches);
    matches.sort_unstable();
    matches
}

fn collect_patience_matches(
    base: &[Line],
    side: &[Line],
    base_start: usize,
    base_end: usize,
    side_start: usize,
    side_end: usize,
    matches: &mut Vec<(usize, usize)>,
) {
    let mut left_base = base_start;
    let mut left_side = side_start;
    while left_base < base_end && left_side < side_end && base[left_base] == side[left_side] {
        matches.push((left_base, left_side));
        left_base += 1;
        left_side += 1;
    }

    let mut right_base = base_end;
    let mut right_side = side_end;
    let mut suffix = Vec::new();
    while left_base < right_base
        && left_side < right_side
        && base[right_base - 1] == side[right_side - 1]
    {
        right_base -= 1;
        right_side -= 1;
        suffix.push((right_base, right_side));
    }

    if left_base < right_base && left_side < right_side {
        let anchors = unique_lis_anchors(base, left_base, right_base, side, left_side, right_side);
        if anchors.is_empty() {
            collect_lcs_matches(
                base, side, left_base, right_base, left_side, right_side, matches,
            );
        } else {
            let mut current_base = left_base;
            let mut current_side = left_side;
            for (anchor_base, anchor_side) in anchors {
                collect_patience_matches(
                    base,
                    side,
                    current_base,
                    anchor_base,
                    current_side,
                    anchor_side,
                    matches,
                );
                matches.push((anchor_base, anchor_side));
                current_base = anchor_base + 1;
                current_side = anchor_side + 1;
            }
            collect_patience_matches(
                base,
                side,
                current_base,
                right_base,
                current_side,
                right_side,
                matches,
            );
        }
    }

    suffix.reverse();
    matches.extend(suffix);
}

#[derive(Debug, Clone, Copy)]
struct Occurrence {
    count: usize,
    index: usize,
}

fn unique_lis_anchors(
    base: &[Line],
    base_start: usize,
    base_end: usize,
    side: &[Line],
    side_start: usize,
    side_end: usize,
) -> Vec<(usize, usize)> {
    let base_occurrences = line_occurrences(&base[base_start..base_end], base_start);
    let side_occurrences = line_occurrences(&side[side_start..side_end], side_start);
    let mut anchors = Vec::new();

    for (line, base_occurrence) in base_occurrences {
        if base_occurrence.count != 1 {
            continue;
        }
        if let Some(side_occurrence) = side_occurrences.get(&line)
            && side_occurrence.count == 1
        {
            anchors.push((base_occurrence.index, side_occurrence.index));
        }
    }

    anchors.sort_unstable_by_key(|(base_index, side_index)| (*base_index, *side_index));
    longest_increasing_by_side(&anchors)
}

fn line_occurrences(lines: &[Line], offset: usize) -> HashMap<Line, Occurrence> {
    let mut occurrences = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        occurrences
            .entry(line.clone())
            .and_modify(|occurrence: &mut Occurrence| occurrence.count += 1)
            .or_insert(Occurrence {
                count: 1,
                index: offset + index,
            });
    }
    occurrences
}

fn longest_increasing_by_side(anchors: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if anchors.is_empty() {
        return Vec::new();
    }

    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; anchors.len()];

    for (index, &(_, side_index)) in anchors.iter().enumerate() {
        let position = tails.partition_point(|&tail_index| anchors[tail_index].1 < side_index);
        if position > 0 {
            previous[index] = Some(tails[position - 1]);
        }
        if position == tails.len() {
            tails.push(index);
        } else if side_index < anchors[tails[position]].1 {
            tails[position] = index;
        }
    }

    let mut output = Vec::new();
    let mut current = tails.last().copied();
    while let Some(index) = current {
        output.push(anchors[index]);
        current = previous[index];
    }
    output.reverse();
    output
}

fn collect_lcs_matches(
    base: &[Line],
    side: &[Line],
    base_start: usize,
    base_end: usize,
    side_start: usize,
    side_end: usize,
    matches: &mut Vec<(usize, usize)>,
) {
    let base_len = base_end - base_start;
    let side_len = side_end - side_start;
    if base_len == 0 || side_len == 0 {
        return;
    }
    if base_len
        .checked_mul(side_len)
        .is_none_or(|cells| cells > MAX_LCS_CELLS)
    {
        return;
    }

    let width = side_len + 1;
    let mut lengths = vec![0usize; (base_len + 1) * (side_len + 1)];
    for base_offset in (0..base_len).rev() {
        for side_offset in (0..side_len).rev() {
            let index = base_offset * width + side_offset;
            lengths[index] = if base[base_start + base_offset] == side[side_start + side_offset] {
                1 + lengths[(base_offset + 1) * width + side_offset + 1]
            } else {
                lengths[((base_offset + 1) * width) + side_offset]
                    .max(lengths[(base_offset * width) + side_offset + 1])
            };
        }
    }

    let mut base_offset = 0;
    let mut side_offset = 0;
    while base_offset < base_len && side_offset < side_len {
        if base[base_start + base_offset] == side[side_start + side_offset] {
            matches.push((base_start + base_offset, side_start + side_offset));
            base_offset += 1;
            side_offset += 1;
        } else if lengths[((base_offset + 1) * width) + side_offset]
            >= lengths[(base_offset * width) + side_offset + 1]
        {
            base_offset += 1;
        } else {
            side_offset += 1;
        }
    }
}

fn apply_resolved_edit(
    hunks: &mut Vec<MergeHunk>,
    base_lines: &[Line],
    base_pos: &mut usize,
    edit: &EditRegion,
) {
    push_resolved_lines(hunks, &base_lines[*base_pos..edit.start]);
    push_resolved_lines(hunks, &edit.replacement);
    *base_pos = edit.end;
}

fn conflict_group(
    ours_edits: &[EditRegion],
    ours_start: usize,
    theirs_edits: &[EditRegion],
    theirs_start: usize,
    conflict_start: usize,
    initial_end: usize,
) -> (usize, usize, usize) {
    let mut conflict_end = initial_end;
    let mut next_ours = ours_start;
    let mut next_theirs = theirs_start;
    let mut changed = true;

    while changed {
        changed = false;
        while next_ours < ours_edits.len()
            && edit_inside_conflict(&ours_edits[next_ours], conflict_start, conflict_end)
        {
            conflict_end = conflict_end.max(ours_edits[next_ours].end);
            next_ours += 1;
            changed = true;
        }
        while next_theirs < theirs_edits.len()
            && edit_inside_conflict(&theirs_edits[next_theirs], conflict_start, conflict_end)
        {
            conflict_end = conflict_end.max(theirs_edits[next_theirs].end);
            next_theirs += 1;
            changed = true;
        }
    }

    (conflict_end, next_ours, next_theirs)
}

fn edit_inside_conflict(edit: &EditRegion, start: usize, end: usize) -> bool {
    if start == end {
        return edit.is_insertion() && edit.start == start;
    }
    if edit.is_insertion() {
        edit.start > start && edit.start < end
    } else {
        edit.start < end && edit.end > start
    }
}

fn apply_edits_to_range(
    base_lines: &[Line],
    edits: &[EditRegion],
    start: usize,
    end: usize,
) -> Vec<u8> {
    let mut output = Vec::new();
    let mut cursor = start;

    for edit in edits {
        if edit.start > cursor {
            append_lines(&mut output, &base_lines[cursor..edit.start]);
        }
        append_lines(&mut output, &edit.replacement);
        cursor = cursor.max(edit.end);
    }

    if cursor < end {
        append_lines(&mut output, &base_lines[cursor..end]);
    }

    output
}

fn classify_content_conflict(base: &[u8], ours: &[u8], theirs: &[u8]) -> ConflictKind {
    if !base.is_empty() && (ours.is_empty() || theirs.is_empty()) {
        ConflictKind::ModifyDelete
    } else if base.is_empty() {
        ConflictKind::BothInserted
    } else {
        ConflictKind::BothModified
    }
}

fn push_resolved_lines(hunks: &mut Vec<MergeHunk>, lines: &[Line]) {
    if !lines.is_empty() {
        push_resolved_bytes(hunks, lines_to_bytes(lines));
    }
}

fn push_resolved_bytes(hunks: &mut Vec<MergeHunk>, bytes: Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    match hunks.last_mut() {
        Some(MergeHunk::Resolved(existing)) => existing.extend(bytes),
        Some(MergeHunk::Conflict(_)) | None => hunks.push(MergeHunk::Resolved(bytes)),
    }
}

fn push_conflict(
    hunks: &mut Vec<MergeHunk>,
    kind: ConflictKind,
    path: Option<PathBuf>,
    base: Option<Vec<u8>>,
    ours: Option<Vec<u8>>,
    theirs: Option<Vec<u8>>,
) {
    hunks.push(MergeHunk::Conflict(MergeConflict::new(
        kind, path, base, ours, theirs,
    )));
}

fn concat_resolved_hunks(hunks: &[MergeHunk]) -> Vec<u8> {
    let mut output = Vec::new();
    for hunk in hunks {
        if let MergeHunk::Resolved(bytes) = hunk {
            output.extend_from_slice(bytes);
        }
    }
    output
}

fn lines_to_bytes(lines: &[Line]) -> Vec<u8> {
    let mut output = Vec::new();
    append_lines(&mut output, lines);
    output
}

fn append_lines(output: &mut Vec<u8>, lines: &[Line]) {
    for line in lines {
        output.extend_from_slice(line);
    }
}

impl FileMergeStrategy for LineMergeStrategy {
    fn name(&self) -> &'static str {
        "line"
    }

    fn merge(&self, input: &FileMergeInput<'_>) -> StrategyMergeResult {
        StrategyMergeResult::Merged(self.merge_file(input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn merge(base: Option<&[u8]>, ours: Option<&[u8]>, theirs: Option<&[u8]>) -> MergeResult {
        merge_file(&FileMergeInput::new(None, base, ours, theirs))
    }

    fn assert_resolved(result: &MergeResult, expected: &[u8]) {
        assert_eq!(result.strategy(), "line");
        assert_eq!(result.resolved_bytes(), Some(expected));
        assert!(result.conflicts().is_empty());
    }

    fn only_conflict(result: &MergeResult) -> &MergeConflict {
        let conflicts = result.conflicts();
        assert_eq!(conflicts.len(), 1, "expected one conflict: {result:#?}");
        conflicts[0]
    }

    #[test]
    fn fast_paths_choose_unchanged_or_identical_side() {
        assert_resolved(
            &merge(Some(b"a\n"), Some(b"same\n"), Some(b"same\n")),
            b"same\n",
        );
        assert_resolved(
            &merge(Some(b"a\n"), Some(b"a\n"), Some(b"theirs\n")),
            b"theirs\n",
        );
        assert_resolved(
            &merge(Some(b"a\n"), Some(b"ours\n"), Some(b"a\n")),
            b"ours\n",
        );
    }

    #[test]
    fn handles_deleted_files() {
        let deleted = merge(Some(b"a\n"), None, None);
        assert_eq!(deleted.resolved_file(), Some(&MergedFile::Deleted));

        let ours_deleted_theirs_unchanged = merge(Some(b"a\n"), None, Some(b"a\n"));
        assert_eq!(
            ours_deleted_theirs_unchanged.resolved_file(),
            Some(&MergedFile::Deleted)
        );

        let theirs_deleted_ours_unchanged = merge(Some(b"a\n"), Some(b"a\n"), None);
        assert_eq!(
            theirs_deleted_ours_unchanged.resolved_file(),
            Some(&MergedFile::Deleted)
        );
    }

    #[test]
    fn one_sided_file_add_resolves() {
        assert_resolved(&merge(None, Some(b"new\n"), None), b"new\n");
        assert_resolved(&merge(None, None, Some(b"new\n")), b"new\n");
    }

    #[test]
    fn add_add_same_content_resolves_but_different_content_conflicts() {
        assert_resolved(&merge(None, Some(b"new\n"), Some(b"new\n")), b"new\n");

        let result = merge(None, Some(b"ours\n"), Some(b"theirs\n"));
        let conflict = only_conflict(&result);
        assert_eq!(conflict.kind(), ConflictKind::BothAdded);
        assert_eq!(conflict.base(), None);
        assert_eq!(conflict.ours(), Some(b"ours\n".as_slice()));
        assert_eq!(conflict.theirs(), Some(b"theirs\n".as_slice()));
    }

    #[test]
    fn one_sided_line_insert_delete_and_replace_resolve() {
        assert_resolved(
            &merge(Some(b"a\nc\n"), Some(b"a\nb\nc\n"), Some(b"a\nc\n")),
            b"a\nb\nc\n",
        );
        assert_resolved(
            &merge(Some(b"a\nb\nc\n"), Some(b"a\nc\n"), Some(b"a\nb\nc\n")),
            b"a\nc\n",
        );
        assert_resolved(
            &merge(Some(b"a\nb\n"), Some(b"a\nB\n"), Some(b"a\nb\n")),
            b"a\nB\n",
        );
    }

    #[test]
    fn non_overlapping_edits_in_same_file_merge() {
        assert_resolved(
            &merge(
                Some(b"a\nb\nc\nd\n"),
                Some(b"a\nOURS\nc\nd\n"),
                Some(b"a\nb\nc\nTHEIRS\n"),
            ),
            b"a\nOURS\nc\nTHEIRS\n",
        );
    }

    #[test]
    fn identical_overlapping_edits_resolve_once() {
        assert_resolved(
            &merge(
                Some(b"a\nb\nc\n"),
                Some(b"a\nshared\nc\n"),
                Some(b"a\nshared\nc\n"),
            ),
            b"a\nshared\nc\n",
        );
    }

    #[test]
    fn conflicting_overlapping_edits_are_structured_hunks() {
        let result = merge(
            Some(b"a\nb\nc\n"),
            Some(b"a\nours\nc\n"),
            Some(b"a\ntheirs\nc\n"),
        );

        match result.outcome() {
            MergeOutcome::Conflicted { hunks } => {
                assert_eq!(hunks.len(), 3);
                assert_eq!(hunks[0], MergeHunk::Resolved(b"a\n".to_vec()));
                assert_eq!(hunks[2], MergeHunk::Resolved(b"c\n".to_vec()));
                let MergeHunk::Conflict(conflict) = &hunks[1] else {
                    panic!("middle hunk should be conflict")
                };
                assert_eq!(conflict.kind(), ConflictKind::BothModified);
                assert_eq!(conflict.base(), Some(b"b\n".as_slice()));
                assert_eq!(conflict.ours(), Some(b"ours\n".as_slice()));
                assert_eq!(conflict.theirs(), Some(b"theirs\n".as_slice()));
            }
            MergeOutcome::Resolved(_) => panic!("expected conflict"),
        }
    }

    #[test]
    fn modify_delete_line_conflicts() {
        let result = merge(Some(b"a\nb\nc\n"), Some(b"a\nc\n"), Some(b"a\nB\nc\n"));
        let conflict = only_conflict(&result);
        assert_eq!(conflict.kind(), ConflictKind::ModifyDelete);
        assert_eq!(conflict.base(), Some(b"b\n".as_slice()));
        assert_eq!(conflict.ours(), Some(b"".as_slice()));
        assert_eq!(conflict.theirs(), Some(b"B\n".as_slice()));
    }

    #[test]
    fn whole_file_modify_delete_conflicts() {
        let result = merge(Some(b"a\n"), None, Some(b"b\n"));
        let conflict = only_conflict(&result);
        assert_eq!(conflict.kind(), ConflictKind::ModifyDelete);
        assert_eq!(conflict.base(), Some(b"a\n".as_slice()));
        assert_eq!(conflict.ours(), None);
        assert_eq!(conflict.theirs(), Some(b"b\n".as_slice()));
    }

    #[test]
    fn same_insertions_at_same_position_resolve_once() {
        assert_resolved(
            &merge(Some(b"a\nc\n"), Some(b"a\nb\nc\n"), Some(b"a\nb\nc\n")),
            b"a\nb\nc\n",
        );
    }

    #[test]
    fn different_insertions_at_same_position_conflict() {
        let result = merge(
            Some(b"a\nc\n"),
            Some(b"a\nours\nc\n"),
            Some(b"a\ntheirs\nc\n"),
        );
        let conflict = only_conflict(&result);
        assert_eq!(conflict.kind(), ConflictKind::BothInserted);
        assert_eq!(conflict.base(), Some(b"".as_slice()));
        assert_eq!(conflict.ours(), Some(b"ours\n".as_slice()));
        assert_eq!(conflict.theirs(), Some(b"theirs\n".as_slice()));
    }

    #[test]
    fn adjacent_insert_and_modify_merge() {
        assert_resolved(
            &merge(Some(b"a\nb\n"), Some(b"a\ninserted\nb\n"), Some(b"a\nB\n")),
            b"a\ninserted\nB\n",
        );
    }

    #[test]
    fn repeated_lines_do_not_create_false_conflicts() {
        assert_resolved(
            &merge(
                Some(b"a\nx\na\nb\n"),
                Some(b"a\nx\na\nOURS\n"),
                Some(b"a\nTHEIRS\na\nb\n"),
            ),
            b"a\nTHEIRS\na\nOURS\n",
        );
    }

    #[test]
    fn preserves_missing_final_newline() {
        assert_resolved(&merge(Some(b"a\nb"), Some(b"a\nB"), Some(b"A\nb")), b"A\nB");
    }

    #[test]
    fn preserves_crlf_from_chosen_sides() {
        assert_resolved(
            &merge(
                Some(b"a\r\nb\r\n"),
                Some(b"a\r\nB\r\n"),
                Some(b"A\r\nb\r\n"),
            ),
            b"A\r\nB\r\n",
        );
    }

    #[test]
    fn merges_utf8_without_normalizing_bytes() {
        assert_resolved(
            &merge(
                Some("α\nβ\n".as_bytes()),
                Some("α\nOURS\n".as_bytes()),
                Some("THEIRS\nβ\n".as_bytes()),
            ),
            "THEIRS\nOURS\n".as_bytes(),
        );
    }

    #[test]
    fn binary_bytes_return_structured_conflict() {
        let result = merge(Some(b"a\0b"), Some(b"a\0ours"), Some(b"a\0theirs"));
        let conflict = only_conflict(&result);
        assert_eq!(conflict.kind(), ConflictKind::Binary);
        assert_eq!(conflict.base(), Some(b"a\0b".as_slice()));
    }

    #[test]
    fn oversized_text_returns_structured_conflict() {
        let base = vec![b'a'; MAX_TEXT_MERGE_BYTES + 1];
        let mut ours = base.clone();
        let mut theirs = base.clone();
        ours[0] = b'o';
        theirs[0] = b't';

        let result = merge(Some(&base), Some(&ours), Some(&theirs));
        let conflict = only_conflict(&result);

        assert_eq!(conflict.kind(), ConflictKind::TooLarge);
    }

    #[test]
    fn large_repeated_line_fixture_merges_deterministically() {
        let mut base = Vec::new();
        let mut ours = Vec::new();
        let mut theirs = Vec::new();
        for index in 0..200 {
            let line = format!("repeat {index}\n");
            base.extend_from_slice(line.as_bytes());
            if index == 50 {
                ours.extend_from_slice(b"ours 50\n");
            } else {
                ours.extend_from_slice(line.as_bytes());
            }
            if index == 150 {
                theirs.extend_from_slice(b"theirs 150\n");
            } else {
                theirs.extend_from_slice(line.as_bytes());
            }
        }

        let result = merge(Some(&base), Some(&ours), Some(&theirs));
        let merged = result.resolved_bytes().expect("merge should resolve");
        assert!(
            merged
                .windows(b"ours 50\n".len())
                .any(|w| w == b"ours 50\n")
        );
        assert!(
            merged
                .windows(b"theirs 150\n".len())
                .any(|w| w == b"theirs 150\n")
        );
    }

    #[test]
    fn empty_strategy_chain_returns_unsupported_conflict() {
        let result = MergeEngine::new().merge(&FileMergeInput::new(
            Some(Path::new("file.txt")),
            Some(b"base\n"),
            Some(b"ours\n"),
            Some(b"theirs\n"),
        ));
        let conflict = only_conflict(&result);

        assert_eq!(result.strategy(), "none");
        assert_eq!(conflict.kind(), ConflictKind::Unsupported);
        assert_eq!(conflict.path(), Some(Path::new("file.txt")));
    }

    #[test]
    fn default_engine_uses_line_strategy() {
        let result = MergeEngine::default().merge(&FileMergeInput::new(
            None,
            Some(b"a\nb\n"),
            Some(b"A\nb\n"),
            Some(b"a\nB\n"),
        ));

        assert_eq!(result.strategy(), "line");
        assert_eq!(result.resolved_bytes(), Some(b"A\nB\n".as_slice()));
    }

    #[test]
    fn strategy_chain_allows_semantic_strategy_before_line_merge() {
        let engine = MergeEngine::new()
            .with_strategy(MockStrategy::resolved("javascript-semantic", b"semantic\n"))
            .with_strategy(LineMergeStrategy::new());
        let result = engine.merge(&FileMergeInput::new(
            Some(Path::new("file.js")),
            Some(b"let value = 1;\n"),
            Some(b"let value = ours;\n"),
            Some(b"let value = theirs;\n"),
        ));

        assert_eq!(result.strategy(), "javascript-semantic");
        assert_eq!(result.resolved_bytes(), Some(b"semantic\n".as_slice()));
    }

    #[test]
    fn unsupported_semantic_strategy_falls_back_to_line_merge() {
        let engine = MergeEngine::new()
            .with_strategy(MockStrategy::unsupported("javascript-semantic"))
            .with_strategy(LineMergeStrategy::new());
        let result = engine.merge(&FileMergeInput::new(
            Some(Path::new("file.js")),
            Some(b"a\nb\n"),
            Some(b"A\nb\n"),
            Some(b"a\nB\n"),
        ));

        assert_eq!(result.strategy(), "line");
        assert_eq!(result.resolved_bytes(), Some(b"A\nB\n".as_slice()));
    }

    #[derive(Clone)]
    struct MockStrategy {
        name: &'static str,
        result: StrategyMergeResult,
    }

    impl MockStrategy {
        fn resolved(name: &'static str, bytes: &'static [u8]) -> Self {
            Self {
                name,
                result: StrategyMergeResult::Merged(MergeOutcome::Resolved(MergedFile::Present(
                    bytes.to_vec(),
                ))),
            }
        }

        fn unsupported(name: &'static str) -> Self {
            Self {
                name,
                result: StrategyMergeResult::Unsupported(UnsupportedMerge::new("parse failed")),
            }
        }
    }

    impl FileMergeStrategy for MockStrategy {
        fn name(&self) -> &'static str {
            self.name
        }

        fn merge(&self, _input: &FileMergeInput<'_>) -> StrategyMergeResult {
            self.result.clone()
        }
    }
}
