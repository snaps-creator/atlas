//! Current recovery verdicts are independent of historical operation errors.
pub(crate) fn complete_retry(status: &mut String, error: &mut Option<String>,
    inspected: Result<(), String>) -> Result<(), String> {
    match inspected {
        Ok(()) => { *status = "Disconnected".into(); *error = None; Ok(()) }
        Err(current) => {
            *status = "CleanupError".into();
            *error = Some(current.clone());
            Err(current)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cleanup_failure_then_fresh_reusable_baseline_releases_retry() {
        let mut status = "CleanupError".into();
        let mut error = Some("old busy adapter".into());
        assert!(complete_retry(&mut status, &mut error, Err("current route remains".into())).is_err());
        assert_eq!(error.as_deref(), Some("current route remains"));
        // The next OS inspection, not the earlier exception, decides retry.
        for _ in 0..2 {
            complete_retry(&mut status, &mut error, Ok(())).unwrap();
            assert_eq!(status, "Disconnected");
            assert_eq!(error, None);
        }
    }
}
