const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn columns(values: &[f32], width: usize, height: usize) -> Vec<Vec<Option<char>>> {
    let eighths: Vec<usize> = right_aligned(values, width)
        .iter()
        .map(|value| value.map_or(0, |value| scaled(value, height * 8)))
        .collect();
    (0..height)
        .rev()
        .map(|row| {
            eighths
                .iter()
                .map(|filled| cell(filled.saturating_sub(row * 8).min(8)))
                .collect()
        })
        .collect()
}

pub fn spark(values: &[f32], width: usize) -> String {
    right_aligned(values, width)
        .iter()
        .map(|value| value.map_or(' ', |value| cell(scaled(value, 8)).unwrap_or(LEVELS[0])))
        .collect()
}

pub fn level(fraction: f32) -> char {
    LEVELS[scaled(fraction, 8).clamp(1, 8) - 1]
}

pub fn meter(fraction: f32, width: usize) -> (String, String) {
    let halves = scaled(fraction, width * 2);
    let full = halves / 2;
    let mut filled = "━".repeat(full);
    if halves % 2 == 1 {
        filled.push('╸');
    }
    let used = full + halves % 2;
    (filled, "─".repeat(width.saturating_sub(used)))
}

fn right_aligned(values: &[f32], width: usize) -> Vec<Option<f32>> {
    let shown = &values[values.len().saturating_sub(width)..];
    std::iter::repeat_n(None, width - shown.len())
        .chain(shown.iter().copied().map(Some))
        .collect()
}

fn scaled(value: f32, steps: usize) -> usize {
    let value = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let scaled = (value * steps as f32).round() as usize;
    if value > 0.0 { scaled.max(1) } else { scaled }
}

fn cell(eighths: usize) -> Option<char> {
    (eighths > 0).then(|| LEVELS[eighths - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(rows: Vec<Vec<Option<char>>>) -> Vec<String> {
        rows.into_iter()
            .map(|row| row.into_iter().map(|cell| cell.unwrap_or(' ')).collect())
            .collect()
    }

    #[test]
    fn columns_stack_full_blocks_under_a_partial_top() {
        assert_eq!(
            render(columns(&[1.0, 0.5, 0.25, 0.0], 4, 2)),
            vec!["█   ".to_string(), "██▄ ".to_string()]
        );
    }

    #[test]
    fn any_activity_shows_at_least_a_sliver_and_history_is_right_aligned() {
        assert_eq!(
            render(columns(&[0.001], 3, 2)),
            vec!["   ".to_string(), "  ▁".to_string()]
        );
        assert_eq!(spark(&[0.0, 1.0], 4), "  ▁█");
    }

    #[test]
    fn charts_survive_odd_input() {
        assert_eq!(
            render(columns(&[f32::NAN, 7.0, -3.0], 3, 1)),
            vec![" █ ".to_string()]
        );
        assert!(columns(&[0.5; 10], 0, 0).is_empty());
        assert_eq!(spark(&[], 3), "   ");
    }

    #[test]
    fn meters_fill_in_half_steps() {
        assert_eq!(meter(0.5, 4), ("━━".into(), "──".into()));
        assert_eq!(meter(0.4, 4), ("━╸".into(), "──".into()));
        assert_eq!(meter(1.5, 3), ("━━━".into(), String::new()));
        assert_eq!(meter(f32::NAN, 2), (String::new(), "──".into()));
    }

    #[test]
    fn levels_cover_the_range_and_never_vanish() {
        assert_eq!(level(0.0), '▁');
        assert_eq!(level(1.0), '█');
        assert_eq!(level(0.5), '▄');
    }
}
