use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct ListItem {
    pub(in crate::tui) label: String,
    pub(in crate::tui) detail: String,
    pub(in crate::tui) group: Option<String>,
    pub(in crate::tui) dimmed: bool,
    pub(in crate::tui) selectable: bool,
    pub(in crate::tui) marked: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::tui) struct ListState {
    pub(in crate::tui) selected: usize,
    pub(in crate::tui) filter: String,
}

pub(in crate::tui) fn visible_indices(items: &[ListItem], filter: &str) -> Vec<usize> {
    let needle = filter.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            needle.is_empty()
                || item.label.to_lowercase().contains(&needle)
                || item.detail.to_lowercase().contains(&needle)
                || item
                    .group
                    .as_deref()
                    .is_some_and(|group| group.to_lowercase().contains(&needle))
        })
        .map(|(index, _)| index)
        .collect()
}

impl ListState {
    fn choices(&self, items: &[ListItem]) -> Vec<usize> {
        visible_indices(items, &self.filter)
            .into_iter()
            .filter(|index| items[*index].selectable)
            .collect()
    }

    pub(in crate::tui) fn move_by(&mut self, items: &[ListItem], delta: isize) {
        let count = self.choices(items).len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let current = self.selected.min(count - 1) as isize;
        self.selected = (current + delta).clamp(0, count as isize - 1) as usize;
    }

    pub(in crate::tui) fn push_filter(&mut self, _items: &[ListItem], c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    pub(in crate::tui) fn pop_filter(&mut self, _items: &[ListItem]) {
        self.filter.pop();
        self.selected = 0;
    }

    pub(in crate::tui) fn current(&self, items: &[ListItem]) -> Option<usize> {
        let choices = self.choices(items);
        choices
            .get(self.selected.min(choices.len().saturating_sub(1)))
            .copied()
    }

    pub(in crate::tui) fn select_item(&mut self, items: &[ListItem], index: usize) {
        if let Some(position) = self.choices(items).iter().position(|i| *i == index) {
            self.selected = position;
        }
    }
}

pub(in crate::tui) fn draw_list(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    items: &[ListItem],
    state: &ListState,
    focused: bool,
) {
    let visible = visible_indices(items, &state.filter);
    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "No matches",
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }
    let current = state.current(items);
    let accent = Color::Rgb(120, 220, 245);
    let mut rows = Vec::new();
    let mut current_row = 0;
    let mut last_group: Option<&str> = None;
    for index in visible {
        let item = &items[index];
        if let Some(group) = item.group.as_deref()
            && last_group != Some(group)
        {
            rows.push(Line::from(Span::styled(
                group.to_owned(),
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            )));
            last_group = Some(group);
        }
        let is_current = current == Some(index);
        if is_current {
            current_row = rows.len();
        }
        let label_style = if item.dimmed {
            Style::default().fg(Color::DarkGray)
        } else if is_current && focused {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let indent = if item.group.is_some() { "  " } else { "" };
        rows.push(Line::from(vec![
            Span::styled(
                if is_current { "▸ " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::raw(indent),
            Span::styled(
                if item.marked { "● " } else { "  " },
                Style::default().fg(Color::Rgb(110, 220, 130)),
            ),
            Span::styled(item.label.clone(), label_style),
            Span::styled(
                if item.detail.is_empty() {
                    String::new()
                } else {
                    format!("   {}", item.detail)
                },
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    let height = area.height as usize;
    let start = (current_row + 1).saturating_sub(height);
    let shown = rows
        .into_iter()
        .skip(start)
        .take(height)
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(shown), area);
}

#[cfg(test)]
mod tests {
    use super::{ListItem, ListState, draw_list, visible_indices};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn item(label: &str, detail: &str, group: Option<&str>) -> ListItem {
        ListItem {
            label: label.to_owned(),
            detail: detail.to_owned(),
            group: group.map(str::to_owned),
            selectable: true,
            ..ListItem::default()
        }
    }

    fn rendered(items: &[ListItem], state: &ListState, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| draw_list(frame, frame.area(), items, state, true))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn filter_matches_label_detail_and_group_case_insensitively() {
        let items = vec![
            item("GPT OSS 120b", "openai/gpt-oss-120b", Some("groq")),
            item("Gemini Pro", "gemini-pro-latest", Some("Google")),
            item("Qwen", "qwen/qwen3", Some("GROQ")),
        ];
        assert_eq!(visible_indices(&items, ""), vec![0, 1, 2]);
        assert_eq!(visible_indices(&items, "gpt"), vec![0]);
        assert_eq!(visible_indices(&items, "PRO-LATEST"), vec![1]);
        assert_eq!(visible_indices(&items, "groq"), vec![0, 2]);
    }

    #[test]
    fn selection_skips_unselectable_and_clamps_after_filter() {
        let mut items = vec![
            item("alpha", "", None),
            item("beta", "", None),
            item("gamma", "", None),
        ];
        items[1].selectable = false;
        let mut state = ListState::default();
        state.move_by(&items, 1);
        assert_eq!(state.current(&items), Some(2));
        state.move_by(&items, 5);
        assert_eq!(state.current(&items), Some(2));
        state.move_by(&items, -1);
        assert_eq!(state.current(&items), Some(0));
        state.move_by(&items, 1);
        state.push_filter(&items, 'a');
        state.push_filter(&items, 'l');
        assert_eq!(state.current(&items), Some(0));
        state.pop_filter(&items);
        state.pop_filter(&items);
        assert_eq!(state.filter, "");
    }

    #[test]
    fn filter_with_no_matches_has_no_current_item() {
        let items = vec![item("alpha", "", None)];
        let mut state = ListState::default();
        state.push_filter(&items, 'z');
        assert_eq!(state.current(&items), None);
        state.move_by(&items, 1);
        assert_eq!(state.current(&items), None);
    }

    #[test]
    fn draw_list_keeps_selection_visible_when_scrolled() {
        let items = (0..20)
            .map(|index| item(&format!("item-{index:02}"), "", None))
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.move_by(&items, 15);
        let screen = rendered(&items, &state, 30, 5);
        assert!(screen.contains("item-15"), "{screen}");
        assert!(!screen.contains("item-00"), "{screen}");
    }

    #[test]
    fn draw_list_shows_group_headers_and_empty_state() {
        let mut items = vec![
            item("GPT", "openai/gpt", Some("groq")),
            item("Gemini", "gemini", Some("Google")),
        ];
        items[0].marked = true;
        let state = ListState::default();
        let screen = rendered(&items, &state, 40, 8);
        assert!(screen.contains("groq"), "{screen}");
        assert!(screen.contains("Google"), "{screen}");
        assert!(screen.contains('●'), "{screen}");
        assert!(screen.contains('▸'), "{screen}");

        let mut filtered = ListState::default();
        filtered.push_filter(&items, 'z');
        let empty = rendered(&items, &filtered, 40, 8);
        assert!(empty.contains("No matches"), "{empty}");
    }
}
