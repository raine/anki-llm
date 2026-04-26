use std::time::Duration;

/// Retry `f` with exponential backoff (1s, 2s, 4s, …, capped at 30s).
///
/// `f` is called up to `max_retries + 1` times. On each error, `is_fatal` is
/// checked — if it returns true the error is returned immediately without
/// further retries. Returns `Ok(T)` on first success, or `Err(last_error)`
/// once all retries are exhausted.
pub fn retry_with_backoff<T, E, F>(
    max_retries: u32,
    is_fatal: impl Fn(&E) -> bool,
    mut f: F,
) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
    E: std::fmt::Display,
{
    let mut last_err: Option<E> = None;

    for attempt in 0..=max_retries {
        if attempt > 0 {
            let delay = Duration::from_millis(1000 * 2u64.pow((attempt - 1).min(5)))
                .min(Duration::from_secs(30));
            eprintln!(
                "  Retry {attempt}/{max_retries}: {}",
                last_err.as_ref().unwrap()
            );
            std::thread::sleep(delay);
        }

        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if is_fatal(&e) {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn succeeds_on_first_try() {
        let result: Result<i32, String> = retry_with_backoff(3, |_| false, || Ok(42));
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn retries_until_success() {
        let mut calls = 0u32;
        let result: Result<i32, String> = retry_with_backoff(
            3,
            |_| false,
            || {
                calls += 1;
                if calls < 3 {
                    Err("transient".into())
                } else {
                    Ok(calls as i32)
                }
            },
        );
        assert_eq!(result.unwrap(), 3);
        assert_eq!(calls, 3);
    }

    #[test]
    fn fatal_error_stops_immediately() {
        let mut calls = 0u32;
        let result: Result<i32, &str> = retry_with_backoff(
            3,
            |e| *e == "fatal",
            || {
                calls += 1;
                Err("fatal")
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn exhausts_retries() {
        let mut calls = 0u32;
        let result: Result<i32, String> = retry_with_backoff(
            2,
            |_| false,
            || {
                calls += 1;
                Err("fail".into())
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, 3); // 1 initial + 2 retries
    }
}
