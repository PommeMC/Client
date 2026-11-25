//! Vanilla `ClientCommonPacketListenerImpl.PackConfirmScreen`: the prompt a
//! server's resource pack raises unless the server is set to Enabled.

use crate::chat_component::{Argument, Component};
use crate::renderer::pipelines::menu_overlay::MenuElement;
use crate::resources::packs::ServerPackRequest;
use crate::ui::common;
use crate::ui::confirm_screen::{CONFIRM_BUTTON_WIDTH, ConfirmScreen};
use crate::ui::text::TextSpan;

/// `ChatFormatting.YELLOW`.
const YELLOW: u32 = 0xFFFF55;

pub struct PackPrompt {
    /// Vanilla `PendingRequest`s, pushed once the prompt is answered.
    pub requests: Vec<ServerPackRequest>,
    pub required: bool,
    /// The server's own message under the question.
    prompt: Option<Component>,
    /// Set once a button or Escape answers it.
    pub answer: Option<bool>,
    /// The last frame's buttons, which a click is tested against.
    buttons: Vec<[f32; 4]>,
}

impl PackPrompt {
    /// Vanilla `addOrUpdatePackPrompt`: an open prompt takes the request on,
    /// with this push's `required` and message.
    pub fn add_or_update(
        slot: &mut Option<Self>,
        request: ServerPackRequest,
        required: bool,
        prompt: Option<Component>,
    ) {
        let mut requests = slot.take().map(|open| open.requests).unwrap_or_default();
        requests.push(request);
        *slot = Some(Self {
            requests,
            required,
            prompt,
            answer: None,
            buttons: Vec::new(),
        });
    }

    fn confirm_screen(&self) -> ConfirmScreen {
        let (title, line2, yes, no) = if self.required {
            (
                "multiplayer.requiredTexturePrompt.line1",
                "multiplayer.requiredTexturePrompt.line2",
                "gui.proceed",
                "menu.disconnect",
            )
        } else {
            (
                "multiplayer.texturePrompt.line1",
                "multiplayer.texturePrompt.line2",
                "gui.yes",
                "gui.no",
            )
        };
        let mut header = Component::translate(line2, Vec::new());
        if self.required {
            header.style.color = Some(YELLOW);
            header.style.bold = Some(true);
        }
        // `preparePackPrompt`.
        let message = match &self.prompt {
            Some(prompt) => Component::translate(
                "multiplayer.texturePrompt.serverPrompt",
                vec![
                    Argument::Component(Box::new(header)),
                    Argument::Component(Box::new(prompt.clone())),
                ],
            ),
            None => header,
        };
        ConfirmScreen {
            title: Component::translate(title, Vec::new()),
            message,
            warning: None,
            buttons: vec![(yes, CONFIRM_BUTTON_WIDTH), (no, CONFIRM_BUTTON_WIDTH)],
        }
    }

    /// Draws the prompt; a click on one of last frame's buttons answers it.
    /// Whether a button was clicked.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        elements: &mut Vec<MenuElement>,
        screen_w: f32,
        screen_h: f32,
        gs: f32,
        cursor: (f32, f32),
        clicked: bool,
        width0: &dyn Fn(&[TextSpan]) -> f32,
    ) -> bool {
        let pressed = clicked
            .then(|| {
                self.buttons
                    .iter()
                    .position(|rect| common::hit_test(cursor, *rect))
            })
            .flatten();
        self.buttons = self
            .confirm_screen()
            .push(elements, cursor, screen_w, screen_h, gs, width0);
        if let Some(button) = pressed {
            self.answer = Some(button == 0);
        }
        pressed.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(n: u128) -> ServerPackRequest {
        ServerPackRequest {
            id: uuid::Uuid::from_u128(n),
            url: "https://example.com/pack.zip".to_owned(),
            hash: String::new(),
        }
    }

    /// A second push joins the open prompt and its flags win.
    #[test]
    fn a_second_push_joins_the_open_prompt() {
        let mut slot = None;
        PackPrompt::add_or_update(&mut slot, request(1), false, None);
        PackPrompt::add_or_update(&mut slot, request(2), true, None);
        let prompt = slot.unwrap();
        let ids: Vec<_> = prompt.requests.iter().map(|r| r.id.as_u128()).collect();
        assert_eq!(ids, [1, 2]);
        assert!(prompt.required);
    }

    /// A required pack asks to proceed or disconnect, its warning yellow and
    /// bold; an optional one asks yes or no.
    #[test]
    fn required_changes_the_question_and_buttons() {
        let mut slot = None;
        PackPrompt::add_or_update(&mut slot, request(1), true, None);
        let screen = slot.as_ref().unwrap().confirm_screen();
        assert_eq!(screen.buttons[0].0, "gui.proceed");
        assert_eq!(screen.buttons[1].0, "menu.disconnect");
        assert_eq!(screen.message.style.color, Some(YELLOW));
        assert_eq!(screen.message.style.bold, Some(true));

        PackPrompt::add_or_update(&mut slot, request(2), false, Some(Component::text("hi")));
        let screen = slot.unwrap().confirm_screen();
        assert_eq!(screen.buttons[0].0, "gui.yes");
        assert_eq!(screen.buttons[1].0, "gui.no");
        let crate::chat_component::Content::Translate { key, args, .. } = &screen.message.content
        else {
            panic!("expected the server prompt wrapper");
        };
        assert_eq!(key, "multiplayer.texturePrompt.serverPrompt");
        assert_eq!(args.len(), 2);
    }
}
