//! Human-readable display only. Protocols, sorting and journals retain integer milliseconds.
pub fn elapsed(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    if ms < 60_000 {
        return format!("{}.{:02} s", ms / 1000, (ms % 1000) / 10);
    }
    let seconds = ms / 1000;
    if seconds < 3600 {
        format!("{} min {} s", seconds / 60, seconds % 60)
    } else {
        format!(
            "{} h {} min {} s",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        )
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn elapsed_units_are_stable_at_boundaries() {
        for (ms, text) in [
            (0, "0 ms"),
            (999, "999 ms"),
            (1000, "1.00 s"),
            (5012, "5.01 s"),
            (59999, "59.99 s"),
            (60000, "1 min 0 s"),
            (135000, "2 min 15 s"),
            (3600000, "1 h 0 min 0 s"),
        ] {
            assert_eq!(super::elapsed(ms), text);
        }
    }
}
