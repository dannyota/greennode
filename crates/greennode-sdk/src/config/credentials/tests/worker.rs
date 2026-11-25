use super::*;

#[test]
fn ended_workers_do_not_read_credentials() {
    use std::io::Seek;
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = Fixture::new("[default]\nclient_id=toy\nclient_secret=toy\n");
    for state in [2, 0, 1, 3] {
        let (ctx, cancel) = crate::RequestContext::new();
        let ctx = if state == 1 {
            ctx.child_with_deadline(tokio::time::Instant::now())
        } else {
            ctx
        };
        if state == 0 {
            cancel.cancel();
        }
        let ended = AtomicBool::new(state == 2);
        let file = fixture.file();
        let mut observer = file.try_clone().unwrap();
        let result = select_checked(
            &fixture.selection(Mode::Greennode),
            &settings("default"),
            &SettingsOptions::default(),
            &Options::default(),
            &Map::default(),
            Some(file),
            &|| {
                ctx.check(&crate::SystemClock).or_else(|| {
                    ended
                        .load(Ordering::Acquire)
                        .then_some(crate::ContextEnd::Cancelled)
                })
            },
        );
        if state == 3 {
            assert!(result.is_ok());
            assert!(observer.stream_position().unwrap() > 0);
        } else {
            let expected = if state == 1 {
                crate::ErrorClass::DeadlineExceeded
            } else {
                crate::ErrorClass::Cancelled
            };
            assert!(matches!(result, Err(error) if error.class() == expected));
            assert_eq!(observer.stream_position().unwrap(), 0);
        }
    }
}
