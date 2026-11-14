//! The account screen: who's signed in, or Microsoft's sign-in: through the
//! browser on desktop, or a code to enter at microsoft.com/link (on phones,
//! and as desktop's fallback).

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

const ACCOUNT_BTN_W: f32 = 150.0;
const CODE_SCALE: f32 = 3.0;

/// Where a sign-in started from this screen has got to.
#[derive(Default)]
pub(super) enum SignIn {
    #[default]
    Idle,
    Requesting,
    /// Waiting on the browser, opened at this sign-in page.
    Browser(String),
    Code(pomme_auth::DeviceCode),
    Failed(String),
    /// `by_player` when they started it, rather than the stored account's
    /// restore at launch.
    Done {
        account: pomme_auth::AuthAccount,
        by_player: bool,
    },
}

/// One sign-in attempt's way to report: it only lands while the attempt is
/// still the newest.
#[derive(Clone)]
pub(super) struct SignInProgress {
    state: Arc<parking_lot::RwLock<SignIn>>,
    current: Arc<AtomicU64>,
    attempt: u64,
}

impl SignInProgress {
    fn set(&self, step: SignIn) {
        // Under the lock, so a newer attempt can't start between the check
        // and the write.
        let mut state = self.state.write();
        if self.current.load(Ordering::Relaxed) == self.attempt {
            *state = step;
        }
    }
}

impl MainMenu {
    pub(super) fn open_account(&mut self, key_bindings: &KeyBindings) {
        self.cancel_sign_in();
        self.set_screen(Screen::Account, key_bindings);
    }

    fn start_sign_in(&mut self) {
        if cfg!(mobile) {
            self.start_code_sign_in();
            return;
        }
        let store = Arc::clone(&self.account_store);
        self.run_sign_in(true, |progress| async move {
            pomme_auth::browser_sign_in(crate::net::http::client(), &*store, |url| {
                crate::util::open_url(url);
                progress.set(SignIn::Browser(url.to_owned()));
            })
            .await
            .map(Some)
        });
    }

    fn start_code_sign_in(&mut self) {
        let store = Arc::clone(&self.account_store);
        self.run_sign_in(true, |progress| async move {
            pomme_auth::device_code_sign_in(crate::net::http::client(), &*store, |code| {
                progress.set(SignIn::Code(code.clone()));
            })
            .await
            .map(Some)
        });
    }

    /// Still signed out, with no sign-in under way: the stored account is
    /// missing or failed to restore.
    pub(crate) fn no_account_to_restore(&self) -> bool {
        self.account_gated() && matches!(*self.sign_in.read(), SignIn::Idle | SignIn::Failed(_))
    }

    /// Signs in as the stored account without asking; with none, the
    /// screen offers Sign In.
    pub(crate) fn restore_account(&mut self) {
        let store = Arc::clone(&self.account_store);
        self.run_sign_in(false, |_| async move {
            Ok(crate::account::restore(&*store).await)
        });
    }

    fn run_sign_in<F>(&mut self, by_player: bool, sign_in: impl FnOnce(SignInProgress) -> F)
    where
        F: Future<Output = Result<Option<pomme_auth::AuthAccount>, String>> + Send + 'static,
    {
        let progress = self.new_attempt(SignIn::Requesting);
        let task = sign_in(progress.clone());
        self.sign_in_task = Some(self.rt.spawn(async move {
            progress.set(match task.await {
                Ok(Some(account)) => SignIn::Done { account, by_player },
                Ok(None) => SignIn::Idle,
                Err(reason) => SignIn::Failed(reason),
            });
        }));
    }

    /// Stops the running attempt and starts a new one at `step`, dropping
    /// every earlier one's progress: an aborted task still finishes the poll
    /// it's in, and could otherwise report after a Cancel or a newer Sign In.
    fn new_attempt(&mut self, step: SignIn) -> SignInProgress {
        if let Some(task) = self.sign_in_task.take() {
            task.abort();
        }
        let mut state = self.sign_in.write();
        let attempt = self.sign_in_attempt.fetch_add(1, Ordering::Relaxed) + 1;
        *state = step;
        self.focus = None;
        SignInProgress {
            state: Arc::clone(&self.sign_in),
            current: Arc::clone(&self.sign_in_attempt),
            attempt,
        }
    }

    pub(crate) fn forget_account(&self, uuid: &uuid::Uuid) {
        crate::account::forget(&*self.account_store, uuid);
    }

    fn close_account(&mut self, key_bindings: &KeyBindings) {
        self.cancel_sign_in();
        self.set_screen(Screen::Main, key_bindings);
    }

    fn cancel_sign_in(&mut self) {
        self.new_attempt(SignIn::Idle);
    }

    pub(super) fn build_account(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        input: &MenuInput,
        key_bindings: &KeyBindings,
    ) -> MainMenuResult {
        let gated = self.account_gated();
        let leave = if gated { "Quit" } else { "Back" };
        if input.escape && !gated {
            self.close_account(key_bindings);
            return empty_result();
        }
        if matches!(*self.sign_in.read(), SignIn::Done { .. }) {
            let SignIn::Done { account, by_player } = std::mem::take(&mut *self.sign_in.write())
            else {
                unreachable!()
            };
            self.sign_in_task = None;
            self.set_screen(Screen::Main, key_bindings);
            let mut result = empty_result();
            result.action = MenuAction::Account {
                account: Some(account),
                by_player,
            };
            return result;
        }

        let (lines, code, buttons): (Vec<String>, Option<String>, Vec<&str>) =
            match (&self.access_token, &*self.sign_in.read()) {
                (Some(_), _) => (
                    vec![format!("Signed in as {}", self.username)],
                    None,
                    vec!["Sign Out", "Back"],
                ),
                (None, SignIn::Idle) if gated => (
                    vec![
                        "Pomme needs a Minecraft account.".into(),
                        "Sign in with the Microsoft account that owns Minecraft.".into(),
                    ],
                    None,
                    vec!["Sign In", leave],
                ),
                (None, SignIn::Idle) => (
                    vec!["Sign in with a Microsoft account to join online servers.".into()],
                    None,
                    vec!["Sign In", leave],
                ),
                (None, SignIn::Requesting) => {
                    (vec!["Contacting Microsoft...".into()], None, vec!["Cancel"])
                }
                (None, SignIn::Browser(_)) => (
                    vec![
                        "Finish signing in in your browser.".into(),
                        "Pomme will continue once you're done.".into(),
                    ],
                    None,
                    vec!["Open Again", "Use a Code", "Cancel"],
                ),
                (None, SignIn::Code(code)) => (
                    vec![format!(
                        "Go to {} and enter this code:",
                        code.verification_uri
                    )],
                    Some(code.user_code.clone()),
                    // TODO: Android's clipboard, through JNI; `set_clipboard`
                    // can't copy there yet.
                    if cfg!(target_os = "android") {
                        vec!["Open Link", "Cancel"]
                    } else {
                        vec!["Open Link", "Copy Code", "Cancel"]
                    },
                ),
                (None, SignIn::Failed(reason)) => (
                    vec!["Sign-in failed:".into(), reason.clone()],
                    None,
                    if cfg!(mobile) {
                        vec!["Try Again", leave]
                    } else {
                        vec!["Try Again", "Use a Code", leave]
                    },
                ),
                (None, SignIn::Done { .. }) => unreachable!("handled above"),
            };

        let gs = self.gui_scale(screen_w, screen_h);
        let fs = common::FONT_SIZE * gs;
        let code_fs = fs * CODE_SCALE;
        let btn_h = common::BTN_H * gs;
        let gap = BTN_GAP * gs;
        let n = buttons.len() as f32;
        // Narrower than usual where the row wouldn't fit the window.
        let btn_w = ((screen_w - (n + 1.0) * gap) / n).min(ACCOUNT_BTN_W * gs);
        let spacing = 8.0 * gs;
        let cx = common::gui_half_width(screen_w, gs);
        let code_h = code.as_ref().map_or(0.0, |_| code_fs + spacing);
        let column_h = (fs + spacing) * lines.len() as f32 + code_h + 2.0 * spacing + btn_h;
        let mut y = common::gui_centered(screen_h, column_h / gs, gs);

        let mut elements = vec![MenuElement::BlurBackground];
        let mut any_hovered = false;
        for line in lines {
            elements.push(MenuElement::Text {
                x: cx,
                y,
                text: line,
                scale: fs,
                color: WHITE,
                centered: true,
            });
            y += fs + spacing;
        }
        if let Some(code) = code {
            elements.push(MenuElement::Text {
                x: cx,
                y,
                text: code,
                scale: code_fs,
                color: WHITE,
                centered: true,
            });
            y += code_fs + spacing;
        }
        y += 2.0 * spacing;

        self.focus_advance(input);
        let mut ctx = self.make_focus_ctx(input);
        let mut pressed = None;
        let row_x = common::gui_centered(screen_w, (n * btn_w + (n - 1.0) * gap) / gs, gs);
        for (i, label) in buttons.into_iter().enumerate() {
            let x = row_x + (i as f32) * (btn_w + gap);
            if push_button_f(
                &mut elements,
                &mut ctx,
                &mut any_hovered,
                input.cursor,
                input.clicked,
                x,
                y,
                btn_w,
                btn_h,
                gs,
                label,
                true,
            ) {
                pressed = Some(label);
            }
        }
        self.finish_focus(&ctx);

        let mut action = MenuAction::None;
        match pressed {
            Some("Sign In" | "Try Again") => self.start_sign_in(),
            Some("Use a Code") => self.start_code_sign_in(),
            Some("Open Again") => {
                if let SignIn::Browser(url) = &*self.sign_in.read() {
                    crate::util::open_url(url);
                }
            }
            Some("Copy Code") => {
                if let SignIn::Code(code) = &*self.sign_in.read() {
                    common::set_clipboard(&code.user_code);
                }
            }
            Some("Open Link") => {
                if let SignIn::Code(code) = &*self.sign_in.read() {
                    crate::util::open_url(&code.verification_uri);
                }
            }
            Some("Sign Out") => {
                action = MenuAction::Account {
                    account: None,
                    by_player: true,
                }
            }
            Some("Cancel") => self.cancel_sign_in(),
            Some("Back") => self.close_account(key_bindings),
            Some("Quit") => action = MenuAction::Quit,
            _ => {}
        }

        MainMenuResult {
            elements,
            action,
            cursor_pointer: any_hovered,
            clicked_button: (input.clicked && any_hovered) || ctx.fired,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::pipelines::menu_overlay::ICON_USER;

    const W: f32 = 1200.0;
    const H: f32 = 800.0;

    fn text_width(text: &str, scale: f32) -> f32 {
        text.len() as f32 * scale * 0.5
    }

    fn texts(result: &MainMenuResult) -> Vec<&str> {
        result
            .elements
            .iter()
            .filter_map(|e| match e {
                MenuElement::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_person_icon_opens_the_account_screen() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-account-icon-test", &mut key_bindings);
        menu.theme = PanoramaTheme::Pomme;
        let main = menu.build(W, H, &MenuInput::default(), &mut key_bindings, text_width);
        let icon = main
            .elements
            .iter()
            .find_map(|e| match e {
                MenuElement::Icon { x, y, icon, .. } if *icon == ICON_USER => Some((*x, *y)),
                _ => None,
            })
            .expect("the main screen draws the person icon");
        let click = MenuInput {
            cursor: icon,
            clicked: true,
            ..Default::default()
        };
        menu.build(W, H, &click, &mut key_bindings, text_width);
        let account = menu.build(W, H, &MenuInput::default(), &mut key_bindings, text_width);
        assert!(texts(&account).contains(&"Sign In"));
        assert!(texts(&account).contains(&"Back"));
    }

    #[test]
    fn a_browser_sign_in_waits_with_a_way_back_and_a_code_fallback() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-browser-sign-in-test", &mut key_bindings);
        menu.open_account(&key_bindings);
        menu.new_attempt(SignIn::Browser("https://login.example/authorize".into()));
        let screen = menu.build(W, H, &MenuInput::default(), &mut key_bindings, text_width);
        for text in [
            "Finish signing in in your browser.",
            "Open Again",
            "Use a Code",
            "Cancel",
        ] {
            assert!(texts(&screen).contains(&text), "{text}");
        }
    }

    #[test]
    fn a_superseded_sign_in_reports_nothing() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-sign-in-race-test", &mut key_bindings);
        let stale = menu.new_attempt(SignIn::Requesting);
        menu.cancel_sign_in();
        stale.set(SignIn::Failed("late".into()));
        assert!(matches!(*menu.sign_in.read(), SignIn::Idle));

        let replaced = menu.new_attempt(SignIn::Requesting);
        let current = menu.new_attempt(SignIn::Requesting);
        replaced.set(SignIn::Failed("late".into()));
        assert!(matches!(*menu.sign_in.read(), SignIn::Requesting));
        current.set(SignIn::Failed("now".into()));
        assert!(matches!(*menu.sign_in.read(), SignIn::Failed(_)));
    }

    #[test]
    fn a_required_account_holds_the_menu_until_sign_in() {
        let mut key_bindings = KeyBindings::default();
        let mut menu = test_menu("pomme-account-gate-test", &mut key_bindings);
        menu.require_account(&key_bindings);
        let escape = MenuInput {
            escape: true,
            ..Default::default()
        };
        menu.build(W, H, &escape, &mut key_bindings, text_width);
        let gate = menu.build(W, H, &MenuInput::default(), &mut key_bindings, text_width);
        assert!(texts(&gate).contains(&"Pomme needs a Minecraft account."));
        assert!(texts(&gate).contains(&"Quit"));
        assert!(!texts(&gate).contains(&"Back"));

        menu.set_account("Tester".into(), Some("token".into()), &key_bindings);
        menu.set_screen(Screen::Main, &key_bindings);
        menu.set_account("Tester".into(), None, &key_bindings);
        assert!(matches!(menu.screen, Screen::Account));
    }
}
