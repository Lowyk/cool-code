//! A collapsible provider → creator → model tree, shared by Settings > Models and `/model`.

use crate::endpoints::model_tags;
use crate::tui::creators::{creator_of, native_creator, strip_group_word};
use crate::tui::models::model_name;
use crate::tui::series::version_of;
use crate::{ProviderProfile, Settings};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::HashSet;

/// Models shown per creator before the rest fold into a "more" row.
const MODELS_PER_CREATOR: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum RowKind {
    Provider,
    Creator,
    Model,
    More,
}

/// The model a row stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct ModelTarget {
    pub(in crate::tui) provider: usize,
    pub(in crate::tui) id: String,
    /// Position in the provider's model list, `None` for a provider that only names one model.
    pub(in crate::tui) index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct TreeRow {
    pub(in crate::tui) depth: u8,
    pub(in crate::tui) kind: RowKind,
    /// Identifies a foldable row in `TreeState::toggled`.
    pub(in crate::tui) key: String,
    pub(in crate::tui) label: String,
    pub(in crate::tui) detail: String,
    pub(in crate::tui) open: bool,
    /// Holds (or is) the active model.
    pub(in crate::tui) marked: bool,
    pub(in crate::tui) dimmed: bool,
    pub(in crate::tui) target: Option<ModelTarget>,
    pub(in crate::tui) parent: Option<usize>,
}

impl TreeRow {
    fn foldable(&self) -> bool {
        self.kind != RowKind::Model
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct TreeState {
    /// Keys the user flipped away from their default (providers start closed unless active,
    /// creators start open, "more" rows start folded).
    toggled: HashSet<String>,
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) filter: String,
}

struct Entry {
    id: String,
    name: String,
    index: Option<usize>,
}

fn entries(profile: &ProviderProfile) -> Vec<Entry> {
    if profile.models.is_empty() {
        return (!profile.model.is_empty())
            .then(|| Entry {
                id: profile.model.clone(),
                name: String::new(),
                index: None,
            })
            .into_iter()
            .collect();
    }
    profile
        .models
        .iter()
        .enumerate()
        .map(|(index, model)| Entry {
            id: model.id.clone(),
            name: model.name.clone(),
            index: Some(index),
        })
        .collect()
}

fn count_label(count: usize) -> String {
    format!("{count} model{}", if count == 1 { "" } else { "s" })
}

fn model_label(entry: &Entry, group: Option<&str>) -> String {
    let pretty = if entry.name.trim().is_empty() {
        let generated = model_name(&entry.id);
        group.map_or(generated.clone(), |group| {
            strip_group_word(&generated, group)
        })
    } else {
        entry.name.clone()
    };
    if pretty.eq_ignore_ascii_case(&entry.id) {
        pretty
    } else {
        format!("{pretty} ({})", entry.id)
    }
}

impl TreeState {
    fn is_active(settings: &Settings, profile: &ProviderProfile, id: &str) -> bool {
        settings.active_provider_id.as_deref() == Some(profile.id.as_str())
            && settings.model.as_deref() == Some(id)
    }

    fn open(&self, key: &str, default: bool) -> bool {
        default != self.toggled.contains(key)
    }

    /// The provider that starts unfolded: the active one, else the first usable one.
    fn default_provider(settings: &Settings, include_drafts: bool) -> Option<usize> {
        let usable = |profile: &ProviderProfile| include_drafts || !profile.draft;
        settings
            .providers
            .iter()
            .position(|profile| {
                usable(profile) && settings.active_provider_id.as_deref() == Some(&profile.id)
            })
            .or_else(|| settings.providers.iter().position(usable))
    }

    pub(in crate::tui) fn rows(&self, settings: &Settings, include_drafts: bool) -> Vec<TreeRow> {
        if self.filter.is_empty() {
            self.tree_rows(settings, include_drafts)
        } else {
            self.filtered_rows(settings, include_drafts)
        }
    }

    fn filtered_rows(&self, settings: &Settings, include_drafts: bool) -> Vec<TreeRow> {
        let needle = self.filter.to_lowercase();
        let mut rows = Vec::new();
        for (provider, profile) in settings.providers.iter().enumerate() {
            if profile.draft && !include_drafts {
                continue;
            }
            let creator = native_creator(&profile.id);
            for entry in entries(profile) {
                let label = model_label(&entry, creator);
                let group = creator.map_or_else(|| creator_of(&entry.id), str::to_owned);
                let haystack = format!("{label} {} {group}", profile.name).to_lowercase();
                if !haystack.contains(&needle) {
                    continue;
                }
                rows.push(TreeRow {
                    depth: 0,
                    kind: RowKind::Model,
                    key: String::new(),
                    label,
                    detail: format!(
                        "{} · {group}{}",
                        profile.name,
                        model_tags(profile.model_info.get(&entry.id))
                    ),
                    open: false,
                    marked: Self::is_active(settings, profile, &entry.id),
                    dimmed: profile.draft,
                    target: (!profile.draft).then(|| ModelTarget {
                        provider,
                        id: entry.id.clone(),
                        index: entry.index,
                    }),
                    parent: None,
                });
            }
        }
        rows
    }

    fn tree_rows(&self, settings: &Settings, include_drafts: bool) -> Vec<TreeRow> {
        let default_provider = Self::default_provider(settings, include_drafts);
        let mut rows = Vec::new();
        for (provider, profile) in settings.providers.iter().enumerate() {
            if profile.draft && !include_drafts {
                continue;
            }
            let native = native_creator(&profile.id);
            let all = entries(profile);
            let provider_key = format!("p:{}", profile.id);
            let provider_open = self.open(&provider_key, default_provider == Some(provider));
            let provider_row = rows.len();
            rows.push(TreeRow {
                depth: 0,
                kind: RowKind::Provider,
                key: provider_key,
                label: profile.name.clone(),
                detail: count_label(all.len()),
                open: provider_open,
                marked: all
                    .iter()
                    .any(|entry| Self::is_active(settings, profile, &entry.id)),
                dimmed: profile.draft,
                target: None,
                parent: None,
            });
            if !provider_open {
                continue;
            }
            // A first-party provider lists its models directly; an aggregator groups by creator.
            let mut groups: Vec<(String, Vec<Entry>)> = Vec::new();
            for entry in all {
                let creator = native.map_or_else(|| creator_of(&entry.id), str::to_owned);
                match groups.iter_mut().find(|(name, _)| *name == creator) {
                    Some((_, members)) => members.push(entry),
                    None => groups.push((creator, vec![entry])),
                }
            }
            groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
            for (creator, mut members) in groups {
                members.sort_by(|a, b| {
                    version_of(&b.id)
                        .cmp(&version_of(&a.id))
                        .then_with(|| a.id.cmp(&b.id))
                });
                let (depth, parent, group_open, group_key) = if native.is_some() {
                    (1, provider_row, true, format!("m:{}:", profile.id))
                } else {
                    let key = format!("c:{}:{creator}", profile.id);
                    let open = self.open(&key, true);
                    let row = rows.len();
                    rows.push(TreeRow {
                        depth: 1,
                        kind: RowKind::Creator,
                        key,
                        label: creator.clone(),
                        detail: count_label(members.len()),
                        open,
                        marked: members
                            .iter()
                            .any(|entry| Self::is_active(settings, profile, &entry.id)),
                        dimmed: profile.draft,
                        target: None,
                        parent: Some(provider_row),
                    });
                    (2, row, open, format!("m:{}:{creator}", profile.id))
                };
                if !group_open {
                    continue;
                }
                let show_all = self.toggled.contains(&group_key);
                let total = members.len();
                let mut shown = 0;
                for entry in &members {
                    let active = Self::is_active(settings, profile, &entry.id);
                    // The active model is never folded away.
                    if !show_all && shown >= MODELS_PER_CREATOR && !active {
                        continue;
                    }
                    shown += 1;
                    rows.push(TreeRow {
                        depth,
                        kind: RowKind::Model,
                        key: String::new(),
                        label: model_label(entry, Some(native.unwrap_or(&creator))),
                        detail: model_tags(profile.model_info.get(&entry.id))
                            .trim_start_matches(" · ")
                            .to_owned(),
                        open: false,
                        marked: active,
                        dimmed: profile.draft,
                        target: (!profile.draft).then(|| ModelTarget {
                            provider,
                            id: entry.id.clone(),
                            index: entry.index,
                        }),
                        parent: Some(parent),
                    });
                }
                if total > MODELS_PER_CREATOR && (show_all || shown < total) {
                    rows.push(TreeRow {
                        depth,
                        kind: RowKind::More,
                        key: group_key,
                        label: if show_all {
                            "Show fewer".to_owned()
                        } else {
                            format!("{} more", total - shown)
                        },
                        detail: String::new(),
                        open: show_all,
                        marked: false,
                        dimmed: false,
                        target: None,
                        parent: Some(parent),
                    });
                }
            }
        }
        rows
    }

    pub(in crate::tui) fn current<'a>(&self, rows: &'a [TreeRow]) -> Option<&'a TreeRow> {
        rows.get(self.selected.min(rows.len().saturating_sub(1)))
    }

    pub(in crate::tui) fn move_by(&mut self, rows: &[TreeRow], delta: isize) {
        if rows.is_empty() {
            self.selected = 0;
            return;
        }
        let current = self.selected.min(rows.len() - 1) as isize;
        self.selected = (current + delta).clamp(0, rows.len() as isize - 1) as usize;
    }

    /// Folds or unfolds the selected row; on a model row it does nothing.
    pub(in crate::tui) fn toggle(&mut self, rows: &[TreeRow]) {
        if let Some(row) = self.current(rows).filter(|row| row.foldable()) {
            let key = row.key.clone();
            if !self.toggled.remove(&key) {
                self.toggled.insert(key);
            }
        }
    }

    /// Right arrow: unfold the selected row.
    pub(in crate::tui) fn expand(&mut self, rows: &[TreeRow]) {
        if self
            .current(rows)
            .is_some_and(|row| row.foldable() && !row.open)
        {
            self.toggle(rows);
        }
    }

    /// Left arrow: fold the selected row, or climb to its parent.
    pub(in crate::tui) fn collapse(&mut self, rows: &[TreeRow]) {
        let Some(row) = self.current(rows) else {
            return;
        };
        if row.foldable() && row.open && row.kind != RowKind::More {
            self.toggle(rows);
        } else if let Some(parent) = row.parent {
            self.selected = parent;
        }
    }

    /// Moves the selection to the active model, if it is visible.
    pub(in crate::tui) fn select_active(&mut self, rows: &[TreeRow]) {
        if let Some(position) = rows
            .iter()
            .position(|row| row.kind == RowKind::Model && row.marked)
        {
            self.selected = position;
        }
    }

    pub(in crate::tui) fn push_filter(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    pub(in crate::tui) fn pop_filter(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }
}

/// Records a click for every row `draw_tree` shows in `area` (each row is one step of ↑/↓);
/// `focus` is the key that gives the tree the keyboard focus, when it does not have it.
pub(in crate::tui) fn record_tree(
    hits: &crate::tui::mouse::Hits,
    area: Rect,
    rows: usize,
    selected: usize,
    focus: Option<crossterm::event::KeyCode>,
) {
    use crate::tui::mouse::{Click, Row, line_rect};
    if rows == 0 {
        return;
    }
    let selected = selected.min(rows - 1);
    let start = (selected + 1).saturating_sub(area.height as usize);
    for position in start..rows.min(start + area.height as usize) {
        hits.click(
            line_rect(area, position - start),
            Click::Row(Row::new(position, selected).focus(focus)),
        );
    }
}

pub(in crate::tui) fn draw_tree(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    rows: &[TreeRow],
    selected: usize,
    focused: bool,
) {
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No matches",
                Style::default().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    }
    let accent = crate::tui::theme::accent_bright();
    let selected = selected.min(rows.len() - 1);
    let lines = rows
        .iter()
        .enumerate()
        .map(|(position, row)| {
            let current = position == selected;
            let label_style = if row.dimmed {
                Style::default().fg(Color::DarkGray)
            } else if current && focused {
                Style::default().fg(accent).add_modifier(Modifier::BOLD)
            } else if row.kind == RowKind::Model {
                Style::default().fg(Color::White)
            } else {
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD)
            };
            let fold = match row.kind {
                RowKind::Model => "  ",
                _ if row.open => "▾ ",
                _ => "▸ ",
            };
            let mut spans = vec![
                Span::styled(
                    if current { "› " } else { "  " },
                    Style::default().fg(accent),
                ),
                Span::raw("  ".repeat(usize::from(row.depth))),
                Span::styled(fold, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if row.marked { "● " } else { "  " },
                    Style::default().fg(Color::Rgb(110, 220, 130)),
                ),
                Span::styled(row.label.clone(), label_style),
            ];
            if !row.detail.is_empty() {
                spans.push(Span::styled(
                    format!("   {}", row.detail),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Line::from(spans)
        })
        .collect::<Vec<_>>();
    let height = area.height as usize;
    let start = (selected + 1).saturating_sub(height);
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(start)
                .take(height)
                .collect::<Vec<_>>(),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::{RowKind, TreeRow, TreeState, draw_tree};
    use crate::{ModelProfile, ProviderProfile, Settings};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn provider(id: &str, models: &[&str]) -> ProviderProfile {
        ProviderProfile {
            id: id.to_owned(),
            name: id.to_owned(),
            adapter: "openai-compatible".to_owned(),
            model: models.first().map(|m| (*m).to_owned()).unwrap_or_default(),
            models: models
                .iter()
                .map(|id| ModelProfile {
                    id: (*id).to_owned(),
                    name: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn labels(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| format!("{}{}", "  ".repeat(usize::from(row.depth)), row.label))
            .collect()
    }

    fn multiai_settings() -> Settings {
        Settings {
            providers: vec![
                provider("google", &["gemini-3-flash", "gemini-3-pro"]),
                provider(
                    "multiai",
                    &[
                        "claude-opus-5-5",
                        "claude-fable-5-1",
                        "gemini-3-flash",
                        "gpt-6-astra",
                        "kimi-k3",
                    ],
                ),
            ],
            active_provider_id: Some("multiai".to_owned()),
            model: Some("claude-opus-5-5".to_owned()),
            ..Default::default()
        }
    }

    fn opus_settings(count: usize, active: &str) -> Settings {
        let ids = (1..=count)
            .map(|n| format!("claude-opus-{n}"))
            .collect::<Vec<_>>();
        let refs = ids.iter().map(String::as_str).collect::<Vec<_>>();
        Settings {
            providers: vec![provider("multiai", &refs)],
            active_provider_id: Some("multiai".to_owned()),
            model: Some(active.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn counts_are_pluralized() {
        assert_eq!(super::count_label(1), "1 model");
        assert_eq!(super::count_label(2), "2 models");
    }

    #[test]
    fn only_the_active_provider_starts_open() {
        let rows = TreeState::default().rows(&multiai_settings(), false);
        assert_eq!(rows[0].label, "google");
        assert!(!rows[0].open);
        assert_eq!(rows[1].label, "multiai");
        assert!(rows[1].open && rows[1].marked);
        assert!(!labels(&rows).iter().any(|row| row.contains("gemini-3-pro")));
    }

    #[test]
    fn an_aggregator_groups_by_creator_and_a_creator_provider_lists_directly() {
        let settings = multiai_settings();
        let mut state = TreeState::default();
        let rows = state.rows(&settings, false);
        state.selected = 0;
        state.toggle(&rows);
        let shown = labels(&state.rows(&settings, false));
        assert_eq!(
            shown[..4],
            [
                "google",
                "  3 Flash (gemini-3-flash)",
                "  3 Pro (gemini-3-pro)",
                "multiai"
            ]
            .map(str::to_owned),
            "{shown:#?}"
        );
        for expected in [
            "  Claude",
            "    Opus 5.5 (claude-opus-5-5)",
            "  Moonshot",
            "    Kimi K3 (kimi-k3)",
        ] {
            assert!(shown.iter().any(|row| row == expected), "{shown:#?}");
        }
    }

    #[test]
    fn newest_models_come_first_and_the_active_one_is_never_folded_away() {
        let rows = TreeState::default().rows(&opus_settings(8, "claude-opus-1"), false);
        let models = rows
            .iter()
            .filter(|row| row.kind == RowKind::Model)
            .map(|row| row.label.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            models.len(),
            6,
            "five newest plus the active one: {models:?}"
        );
        assert_eq!(models[0], "Opus 8 (claude-opus-8)");
        assert_eq!(models[5], "Opus 1 (claude-opus-1)");
        let more = rows.iter().find(|row| row.kind == RowKind::More).unwrap();
        assert_eq!(more.label, "2 more");

        let rows = TreeState::default().rows(&opus_settings(8, "claude-opus-8"), false);
        let more = rows.iter().find(|row| row.kind == RowKind::More).unwrap();
        assert_eq!(more.label, "3 more");
    }

    #[test]
    fn more_unfolds_the_rest_and_show_fewer_folds_them_again() {
        let settings = opus_settings(8, "none");
        let mut state = TreeState::default();
        let rows = state.rows(&settings, false);
        state.selected = rows.iter().position(|r| r.kind == RowKind::More).unwrap();
        state.toggle(&rows);
        let rows = state.rows(&settings, false);
        assert_eq!(rows.iter().filter(|r| r.kind == RowKind::Model).count(), 8);
        assert_eq!(rows.last().unwrap().label, "Show fewer");
        state.selected = rows.len() - 1;
        state.toggle(&rows);
        let rows = state.rows(&settings, false);
        assert_eq!(rows.iter().filter(|r| r.kind == RowKind::Model).count(), 5);
    }

    #[test]
    fn arrows_fold_unfold_and_climb() {
        let settings = multiai_settings();
        let mut state = TreeState::default();
        let rows = state.rows(&settings, false);
        state.selected = 0;
        state.expand(&rows);
        assert!(state.rows(&settings, false)[0].open);
        let rows = state.rows(&settings, false);
        state.collapse(&rows);
        assert!(!state.rows(&settings, false)[0].open);
        let rows = state.rows(&settings, false);
        state.selected = rows
            .iter()
            .position(|row| row.kind == RowKind::Model && row.marked)
            .unwrap();
        state.collapse(&rows);
        assert_eq!(rows[state.selected].kind, RowKind::Creator);
        state.collapse(&rows);
        assert!(
            !labels(&state.rows(&settings, false))
                .iter()
                .any(|row| row.contains("claude-opus-5-5"))
        );
    }

    #[test]
    fn filtering_flattens_to_matching_models_with_their_path() {
        let settings = multiai_settings();
        let mut state = TreeState::default();
        for c in "flash".chars() {
            state.push_filter(c);
        }
        let rows = state.rows(&settings, false);
        assert_eq!(rows.len(), 2, "{:?}", labels(&rows));
        assert!(
            rows.iter()
                .all(|row| row.kind == RowKind::Model && row.depth == 0)
        );
        assert!(rows[0].detail.contains("google"));
        assert_eq!(rows[1].target.as_ref().unwrap().provider, 1);
    }

    #[test]
    fn drafts_appear_only_when_asked_for_and_cannot_be_chosen() {
        let mut draft = provider("wip", &["draft-model"]);
        draft.draft = true;
        let settings = Settings {
            providers: vec![provider("multiai", &["kimi-k3"]), draft],
            ..Default::default()
        };
        let mut state = TreeState::default();
        assert!(
            !labels(&state.rows(&settings, false))
                .iter()
                .any(|row| row == "wip")
        );
        let with = state.rows(&settings, true);
        let wip = with.iter().position(|row| row.label == "wip").unwrap();
        assert!(with[wip].dimmed);
        state.selected = wip;
        state.toggle(&with);
        let opened = state.rows(&settings, true);
        let model = opened
            .iter()
            .find(|row| row.label.contains("draft-model"))
            .unwrap();
        assert!(model.dimmed && model.target.is_none());
    }

    #[test]
    fn a_provider_that_names_only_one_model_still_lists_it() {
        let mut single = provider("solo", &[]);
        single.model = "claude-opus-5".to_owned();
        let settings = Settings {
            providers: vec![single],
            ..Default::default()
        };
        let rows = TreeState::default().rows(&settings, false);
        let model = rows.iter().find(|row| row.kind == RowKind::Model).unwrap();
        assert_eq!(model.target.as_ref().unwrap().index, None);
    }

    #[test]
    fn custom_names_are_kept_and_tags_follow_the_row() {
        let mut settings = multiai_settings();
        settings.providers[1].models[0].name = "My Opus".to_owned();
        settings.providers[1].model_info.insert(
            "kimi-k3".to_owned(),
            crate::ModelInfo {
                free: Some(true),
                tools: None,
                context: None,
            },
        );
        let rows = TreeState::default().rows(&settings, false);
        assert!(
            rows.iter()
                .any(|row| row.label == "My Opus (claude-opus-5-5)")
        );
        let kimi = rows
            .iter()
            .find(|row| row.label.contains("kimi-k3"))
            .unwrap();
        assert_eq!(kimi.detail, "free");
    }

    #[test]
    fn drawing_shows_the_cursor_and_keeps_the_selection_in_view() {
        let settings = multiai_settings();
        let rows = TreeState::default().rows(&settings, false);
        let mut terminal = Terminal::new(TestBackend::new(60, 4)).expect("terminal");
        terminal
            .draw(|frame| draw_tree(frame, frame.area(), &rows, rows.len() - 1, true))
            .expect("draw");
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("kimi-k3"), "{screen}");
        assert!(screen.contains('›'), "{screen}");
        assert!(!screen.contains("google"), "{screen}");
    }
}
