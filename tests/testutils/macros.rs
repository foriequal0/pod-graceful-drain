#[macro_export]
macro_rules! eventually {
    ($cond:expr $(,)?) => {{
        eventually!(timeout = 10, $cond)
    }};

    (timeout = $time:literal, $cond:expr $(,)?) => {{
        use ::std::time::Duration;
        use ::tokio::time::{sleep, timeout};

        let result = timeout(Duration::from_secs($time), async {
            loop {
                if async { $cond }.await {
                    return;
                }

                sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        match result {
            Ok(()) => true,
            Err(_) => false,
        }
    }};
}

#[macro_export]
macro_rules! eventually_some {
    ($cond:expr $(,)?) => {{
        eventually_some!(timeout = 10, $cond)
    }};

    (timeout = $time:expr, $cond:expr $(,)?) => {{
        use ::std::time::Duration;
        use ::tokio::time::{sleep, timeout};

        let result = timeout(Duration::from_secs($time), async {
            loop {
                if let Some(res) = async { $cond }.await {
                    return res;
                }

                sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        match result {
            Ok(res) => res,
            Err(_) => panic!("Timeout error: {}", stringify!($cond)),
        }
    }};
}
