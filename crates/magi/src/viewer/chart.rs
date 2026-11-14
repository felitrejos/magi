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
    }

    #[test]
    fn charts_survive_odd_input() {
        assert_eq!(
            render(columns(&[f32::NAN, 7.0, -3.0], 3, 1)),
            vec![" █ ".to_string()]
        );
        assert!(columns(&[0.5; 10], 0, 0).is_empty());
    }
}
