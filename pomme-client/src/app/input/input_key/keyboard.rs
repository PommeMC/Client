use winit::keyboard::KeyCode;

use super::super::StringId;

// Ids and labels follow 26.2 `InputConstants`/en_us; keys 26.2 lacks use
// 26.3's scancode names. These tokens describe positions, not typed text.
input_catalog! {
    input_type: KeyCode,
    input_variant: Keyboard,
    id_builder: keyboard_input_id,
    // KeyCode is non-exhaustive; future keys need a stable name here.
    metadata_fallback: { _ => None, },
    decode_fallback: |_| None,
    entries: {
        Backquote => {
            serialized_suffix: "grave.accent",
            translation_id: Some(StringId::new("key.keyboard.grave.accent")),
            fallback: "`",
        },
        Backslash => {
            serialized_suffix: "backslash",
            translation_id: Some(StringId::new("key.keyboard.backslash")),
            fallback: "\\",
        },
        BracketLeft => {
            serialized_suffix: "left.bracket",
            translation_id: Some(StringId::new("key.keyboard.left.bracket")),
            fallback: "[",
        },
        BracketRight => {
            serialized_suffix: "right.bracket",
            translation_id: Some(StringId::new("key.keyboard.right.bracket")),
            fallback: "]",
        },
        Comma => {
            serialized_suffix: "comma",
            translation_id: Some(StringId::new("key.keyboard.comma")),
            fallback: ",",
        },
        Digit0 => {
            serialized_suffix: "0",
            translation_id: None,
            fallback: "0",
        },
        Digit1 => {
            serialized_suffix: "1",
            translation_id: None,
            fallback: "1",
        },
        Digit2 => {
            serialized_suffix: "2",
            translation_id: None,
            fallback: "2",
        },
        Digit3 => {
            serialized_suffix: "3",
            translation_id: None,
            fallback: "3",
        },
        Digit4 => {
            serialized_suffix: "4",
            translation_id: None,
            fallback: "4",
        },
        Digit5 => {
            serialized_suffix: "5",
            translation_id: None,
            fallback: "5",
        },
        Digit6 => {
            serialized_suffix: "6",
            translation_id: None,
            fallback: "6",
        },
        Digit7 => {
            serialized_suffix: "7",
            translation_id: None,
            fallback: "7",
        },
        Digit8 => {
            serialized_suffix: "8",
            translation_id: None,
            fallback: "8",
        },
        Digit9 => {
            serialized_suffix: "9",
            translation_id: None,
            fallback: "9",
        },
        Equal => {
            serialized_suffix: "equal",
            translation_id: Some(StringId::new("key.keyboard.equal")),
            fallback: "=",
        },
        IntlBackslash => {
            serialized_suffix: "world.1",
            translation_id: Some(StringId::new("key.keyboard.world.1")),
            fallback: "World 1",
        },
        IntlRo => {
            serialized_suffix: "intl.ro",
            translation_id: None,
            fallback: "International Ro",
        },
        IntlYen => {
            serialized_suffix: "intl.yen",
            translation_id: None,
            fallback: "International Yen",
        },
        KeyA => {
            serialized_suffix: "a",
            translation_id: None,
            fallback: "A",
        },
        KeyB => {
            serialized_suffix: "b",
            translation_id: None,
            fallback: "B",
        },
        KeyC => {
            serialized_suffix: "c",
            translation_id: None,
            fallback: "C",
        },
        KeyD => {
            serialized_suffix: "d",
            translation_id: None,
            fallback: "D",
        },
        KeyE => {
            serialized_suffix: "e",
            translation_id: None,
            fallback: "E",
        },
        KeyF => {
            serialized_suffix: "f",
            translation_id: None,
            fallback: "F",
        },
        KeyG => {
            serialized_suffix: "g",
            translation_id: None,
            fallback: "G",
        },
        KeyH => {
            serialized_suffix: "h",
            translation_id: None,
            fallback: "H",
        },
        KeyI => {
            serialized_suffix: "i",
            translation_id: None,
            fallback: "I",
        },
        KeyJ => {
            serialized_suffix: "j",
            translation_id: None,
            fallback: "J",
        },
        KeyK => {
            serialized_suffix: "k",
            translation_id: None,
            fallback: "K",
        },
        KeyL => {
            serialized_suffix: "l",
            translation_id: None,
            fallback: "L",
        },
        KeyM => {
            serialized_suffix: "m",
            translation_id: None,
            fallback: "M",
        },
        KeyN => {
            serialized_suffix: "n",
            translation_id: None,
            fallback: "N",
        },
        KeyO => {
            serialized_suffix: "o",
            translation_id: None,
            fallback: "O",
        },
        KeyP => {
            serialized_suffix: "p",
            translation_id: None,
            fallback: "P",
        },
        KeyQ => {
            serialized_suffix: "q",
            translation_id: None,
            fallback: "Q",
        },
        KeyR => {
            serialized_suffix: "r",
            translation_id: None,
            fallback: "R",
        },
        KeyS => {
            serialized_suffix: "s",
            translation_id: None,
            fallback: "S",
        },
        KeyT => {
            serialized_suffix: "t",
            translation_id: None,
            fallback: "T",
        },
        KeyU => {
            serialized_suffix: "u",
            translation_id: None,
            fallback: "U",
        },
        KeyV => {
            serialized_suffix: "v",
            translation_id: None,
            fallback: "V",
        },
        KeyW => {
            serialized_suffix: "w",
            translation_id: None,
            fallback: "W",
        },
        KeyX => {
            serialized_suffix: "x",
            translation_id: None,
            fallback: "X",
        },
        KeyY => {
            serialized_suffix: "y",
            translation_id: None,
            fallback: "Y",
        },
        KeyZ => {
            serialized_suffix: "z",
            translation_id: None,
            fallback: "Z",
        },
        Minus => {
            serialized_suffix: "minus",
            translation_id: Some(StringId::new("key.keyboard.minus")),
            fallback: "-",
        },
        Period => {
            serialized_suffix: "period",
            translation_id: Some(StringId::new("key.keyboard.period")),
            fallback: ".",
        },
        Quote => {
            serialized_suffix: "apostrophe",
            translation_id: Some(StringId::new("key.keyboard.apostrophe")),
            fallback: "'",
        },
        Semicolon => {
            serialized_suffix: "semicolon",
            translation_id: Some(StringId::new("key.keyboard.semicolon")),
            fallback: ";",
        },
        Slash => {
            serialized_suffix: "slash",
            translation_id: Some(StringId::new("key.keyboard.slash")),
            fallback: "/",
        },
        AltLeft => {
            serialized_suffix: "left.alt",
            translation_id: Some(StringId::new("key.keyboard.left.alt")),
            fallback: "Left Alt",
        },
        AltRight => {
            serialized_suffix: "right.alt",
            translation_id: Some(StringId::new("key.keyboard.right.alt")),
            fallback: "Right Alt",
        },
        Backspace => {
            serialized_suffix: "backspace",
            translation_id: Some(StringId::new("key.keyboard.backspace")),
            fallback: "Backspace",
        },
        CapsLock => {
            serialized_suffix: "caps.lock",
            translation_id: Some(StringId::new("key.keyboard.caps.lock")),
            fallback: "Caps Lock",
        },
        ContextMenu => {
            serialized_suffix: "menu",
            translation_id: Some(StringId::new("key.keyboard.menu")),
            fallback: "Menu",
        },
        ControlLeft => {
            serialized_suffix: "left.control",
            translation_id: Some(StringId::new("key.keyboard.left.control")),
            fallback: "Left Control",
        },
        ControlRight => {
            serialized_suffix: "right.control",
            translation_id: Some(StringId::new("key.keyboard.right.control")),
            fallback: "Right Control",
        },
        Enter => {
            serialized_suffix: "enter",
            translation_id: Some(StringId::new("key.keyboard.enter")),
            fallback: "Enter",
        },
        SuperLeft => {
            serialized_suffix: "left.win",
            translation_id: Some(StringId::new("key.keyboard.left.win")),
            fallback: "Left Win",
        },
        SuperRight => {
            serialized_suffix: "right.win",
            translation_id: Some(StringId::new("key.keyboard.right.win")),
            fallback: "Right Win",
        },
        ShiftLeft => {
            serialized_suffix: "left.shift",
            translation_id: Some(StringId::new("key.keyboard.left.shift")),
            fallback: "Left Shift",
        },
        ShiftRight => {
            serialized_suffix: "right.shift",
            translation_id: Some(StringId::new("key.keyboard.right.shift")),
            fallback: "Right Shift",
        },
        Space => {
            serialized_suffix: "space",
            translation_id: Some(StringId::new("key.keyboard.space")),
            fallback: "Space",
        },
        Tab => {
            serialized_suffix: "tab",
            translation_id: Some(StringId::new("key.keyboard.tab")),
            fallback: "Tab",
        },
        Convert => {
            serialized_suffix: "convert",
            translation_id: None,
            fallback: "Convert",
        },
        KanaMode => {
            serialized_suffix: "kana.mode",
            translation_id: None,
            fallback: "Kana Mode",
        },
        Lang1 => {
            serialized_suffix: "lang1",
            translation_id: Some(StringId::new("key.keyboard.lang1")),
            fallback: "Language 1",
        },
        Lang2 => {
            serialized_suffix: "lang2",
            translation_id: Some(StringId::new("key.keyboard.lang2")),
            fallback: "Language 2",
        },
        Lang3 => {
            serialized_suffix: "lang3",
            translation_id: Some(StringId::new("key.keyboard.lang3")),
            fallback: "Language 3",
        },
        Lang4 => {
            serialized_suffix: "lang4",
            translation_id: Some(StringId::new("key.keyboard.lang4")),
            fallback: "Language 4",
        },
        Lang5 => {
            serialized_suffix: "lang5",
            translation_id: Some(StringId::new("key.keyboard.lang5")),
            fallback: "Language 5",
        },
        NonConvert => {
            serialized_suffix: "non.convert",
            translation_id: None,
            fallback: "Non Convert",
        },
        Delete => {
            serialized_suffix: "delete",
            translation_id: Some(StringId::new("key.keyboard.delete")),
            fallback: "Delete",
        },
        End => {
            serialized_suffix: "end",
            translation_id: Some(StringId::new("key.keyboard.end")),
            fallback: "End",
        },
        Help => {
            serialized_suffix: "help",
            translation_id: Some(StringId::new("key.keyboard.help")),
            fallback: "Help",
        },
        Home => {
            serialized_suffix: "home",
            translation_id: Some(StringId::new("key.keyboard.home")),
            fallback: "Home",
        },
        Insert => {
            serialized_suffix: "insert",
            translation_id: Some(StringId::new("key.keyboard.insert")),
            fallback: "Insert",
        },
        PageDown => {
            serialized_suffix: "page.down",
            translation_id: Some(StringId::new("key.keyboard.page.down")),
            fallback: "Page Down",
        },
        PageUp => {
            serialized_suffix: "page.up",
            translation_id: Some(StringId::new("key.keyboard.page.up")),
            fallback: "Page Up",
        },
        ArrowDown => {
            serialized_suffix: "down",
            translation_id: Some(StringId::new("key.keyboard.down")),
            fallback: "Down Arrow",
        },
        ArrowLeft => {
            serialized_suffix: "left",
            translation_id: Some(StringId::new("key.keyboard.left")),
            fallback: "Left Arrow",
        },
        ArrowRight => {
            serialized_suffix: "right",
            translation_id: Some(StringId::new("key.keyboard.right")),
            fallback: "Right Arrow",
        },
        ArrowUp => {
            serialized_suffix: "up",
            translation_id: Some(StringId::new("key.keyboard.up")),
            fallback: "Up Arrow",
        },
        NumLock => {
            serialized_suffix: "num.lock",
            translation_id: Some(StringId::new("key.keyboard.num.lock")),
            fallback: "Num Lock",
        },
        Numpad0 => {
            serialized_suffix: "keypad.0",
            translation_id: Some(StringId::new("key.keyboard.keypad.0")),
            fallback: "Keypad 0",
        },
        Numpad1 => {
            serialized_suffix: "keypad.1",
            translation_id: Some(StringId::new("key.keyboard.keypad.1")),
            fallback: "Keypad 1",
        },
        Numpad2 => {
            serialized_suffix: "keypad.2",
            translation_id: Some(StringId::new("key.keyboard.keypad.2")),
            fallback: "Keypad 2",
        },
        Numpad3 => {
            serialized_suffix: "keypad.3",
            translation_id: Some(StringId::new("key.keyboard.keypad.3")),
            fallback: "Keypad 3",
        },
        Numpad4 => {
            serialized_suffix: "keypad.4",
            translation_id: Some(StringId::new("key.keyboard.keypad.4")),
            fallback: "Keypad 4",
        },
        Numpad5 => {
            serialized_suffix: "keypad.5",
            translation_id: Some(StringId::new("key.keyboard.keypad.5")),
            fallback: "Keypad 5",
        },
        Numpad6 => {
            serialized_suffix: "keypad.6",
            translation_id: Some(StringId::new("key.keyboard.keypad.6")),
            fallback: "Keypad 6",
        },
        Numpad7 => {
            serialized_suffix: "keypad.7",
            translation_id: Some(StringId::new("key.keyboard.keypad.7")),
            fallback: "Keypad 7",
        },
        Numpad8 => {
            serialized_suffix: "keypad.8",
            translation_id: Some(StringId::new("key.keyboard.keypad.8")),
            fallback: "Keypad 8",
        },
        Numpad9 => {
            serialized_suffix: "keypad.9",
            translation_id: Some(StringId::new("key.keyboard.keypad.9")),
            fallback: "Keypad 9",
        },
        NumpadAdd => {
            serialized_suffix: "keypad.add",
            translation_id: Some(StringId::new("key.keyboard.keypad.add")),
            fallback: "Keypad +",
        },
        NumpadBackspace => {
            serialized_suffix: "keypad.backspace",
            translation_id: Some(StringId::new("key.keyboard.keypad.backspace")),
            fallback: "Keypad Backspace",
        },
        NumpadClear => {
            serialized_suffix: "keypad.clear",
            translation_id: Some(StringId::new("key.keyboard.keypad.clear")),
            fallback: "Keypad Clear",
        },
        NumpadClearEntry => {
            serialized_suffix: "keypad.clear.entry",
            translation_id: Some(StringId::new("key.keyboard.keypad.clear.entry")),
            fallback: "Keypad Clear Entry",
        },
        NumpadComma => {
            serialized_suffix: "keypad.comma",
            translation_id: Some(StringId::new("key.keyboard.keypad.comma")),
            fallback: "Keypad ,",
        },
        NumpadDecimal => {
            serialized_suffix: "keypad.decimal",
            translation_id: Some(StringId::new("key.keyboard.keypad.decimal")),
            fallback: "Keypad Decimal",
        },
        NumpadDivide => {
            serialized_suffix: "keypad.divide",
            translation_id: Some(StringId::new("key.keyboard.keypad.divide")),
            fallback: "Keypad /",
        },
        NumpadEnter => {
            serialized_suffix: "keypad.enter",
            translation_id: Some(StringId::new("key.keyboard.keypad.enter")),
            fallback: "Keypad Enter",
        },
        NumpadEqual => {
            serialized_suffix: "keypad.equal",
            translation_id: Some(StringId::new("key.keyboard.keypad.equal")),
            fallback: "Keypad =",
        },
        NumpadHash => {
            serialized_suffix: "keypad.hash",
            translation_id: Some(StringId::new("key.keyboard.keypad.hash")),
            fallback: "Keypad #",
        },
        NumpadMemoryAdd => {
            serialized_suffix: "keypad.memory.add",
            translation_id: Some(StringId::new("key.keyboard.keypad.memory.add")),
            fallback: "Keypad M+",
        },
        NumpadMemoryClear => {
            serialized_suffix: "keypad.memory.clear",
            translation_id: Some(StringId::new("key.keyboard.keypad.memory.clear")),
            fallback: "Keypad MC",
        },
        NumpadMemoryRecall => {
            serialized_suffix: "keypad.memory.recall",
            translation_id: Some(StringId::new("key.keyboard.keypad.memory.recall")),
            fallback: "Keypad MR",
        },
        NumpadMemoryStore => {
            serialized_suffix: "keypad.memory.store",
            translation_id: Some(StringId::new("key.keyboard.keypad.memory.store")),
            fallback: "Keypad MS",
        },
        NumpadMemorySubtract => {
            serialized_suffix: "keypad.memory.subtract",
            translation_id: Some(StringId::new("key.keyboard.keypad.memory.subtract")),
            fallback: "Keypad M-",
        },
        NumpadMultiply => {
            serialized_suffix: "keypad.multiply",
            translation_id: Some(StringId::new("key.keyboard.keypad.multiply")),
            fallback: "Keypad *",
        },
        NumpadParenLeft => {
            serialized_suffix: "keypad.left.parenthesis",
            translation_id: Some(StringId::new("key.keyboard.keypad.left.parenthesis")),
            fallback: "Keypad (",
        },
        NumpadParenRight => {
            serialized_suffix: "keypad.right.parenthesis",
            translation_id: Some(StringId::new("key.keyboard.keypad.right.parenthesis")),
            fallback: "Keypad )",
        },
        NumpadStar => {
            serialized_suffix: "keypad.star",
            translation_id: None,
            fallback: "Keypad Star",
        },
        NumpadSubtract => {
            serialized_suffix: "keypad.subtract",
            translation_id: Some(StringId::new("key.keyboard.keypad.subtract")),
            fallback: "Keypad -",
        },
        Escape => {
            serialized_suffix: "escape",
            translation_id: Some(StringId::new("key.keyboard.escape")),
            fallback: "Escape",
        },
        Fn => {
            serialized_suffix: "fn",
            translation_id: None,
            fallback: "Fn",
        },
        FnLock => {
            serialized_suffix: "fn.lock",
            translation_id: None,
            fallback: "Fn Lock",
        },
        PrintScreen => {
            serialized_suffix: "print.screen",
            translation_id: Some(StringId::new("key.keyboard.print.screen")),
            fallback: "Print Screen",
        },
        ScrollLock => {
            serialized_suffix: "scroll.lock",
            translation_id: Some(StringId::new("key.keyboard.scroll.lock")),
            fallback: "Scroll Lock",
        },
        Pause => {
            serialized_suffix: "pause",
            translation_id: Some(StringId::new("key.keyboard.pause")),
            fallback: "Pause",
        },
        BrowserBack => {
            serialized_suffix: "ac.back",
            translation_id: Some(StringId::new("key.keyboard.ac.back")),
            fallback: "AC Back",
        },
        BrowserFavorites => {
            serialized_suffix: "ac.bookmarks",
            translation_id: Some(StringId::new("key.keyboard.ac.bookmarks")),
            fallback: "AC Favorites",
        },
        BrowserForward => {
            serialized_suffix: "ac.forward",
            translation_id: Some(StringId::new("key.keyboard.ac.forward")),
            fallback: "AC Forward",
        },
        BrowserHome => {
            serialized_suffix: "ac.home",
            translation_id: Some(StringId::new("key.keyboard.ac.home")),
            fallback: "AC Home",
        },
        BrowserRefresh => {
            serialized_suffix: "ac.refresh",
            translation_id: Some(StringId::new("key.keyboard.ac.refresh")),
            fallback: "AC Refresh",
        },
        BrowserSearch => {
            serialized_suffix: "ac.search",
            translation_id: Some(StringId::new("key.keyboard.ac.search")),
            fallback: "AC Search",
        },
        BrowserStop => {
            serialized_suffix: "ac.stop",
            translation_id: Some(StringId::new("key.keyboard.ac.stop")),
            fallback: "AC Stop",
        },
        Eject => {
            serialized_suffix: "media.eject",
            translation_id: Some(StringId::new("key.keyboard.media.eject")),
            fallback: "Eject",
        },
        LaunchApp1 => {
            serialized_suffix: "launch.app.1",
            translation_id: None,
            fallback: "Launch App 1",
        },
        LaunchApp2 => {
            serialized_suffix: "launch.app.2",
            translation_id: None,
            fallback: "Launch App 2",
        },
        LaunchMail => {
            serialized_suffix: "launch.mail",
            translation_id: None,
            fallback: "Launch Mail",
        },
        MediaPlayPause => {
            serialized_suffix: "media.play.pause",
            translation_id: Some(StringId::new("key.keyboard.media.play.pause")),
            fallback: "Play/Pause",
        },
        MediaSelect => {
            serialized_suffix: "media.select",
            translation_id: Some(StringId::new("key.keyboard.media.select")),
            fallback: "Media Select",
        },
        MediaStop => {
            serialized_suffix: "media.stop",
            translation_id: Some(StringId::new("key.keyboard.media.stop")),
            fallback: "Media Stop",
        },
        MediaTrackNext => {
            serialized_suffix: "media.next.track",
            translation_id: Some(StringId::new("key.keyboard.media.next.track")),
            fallback: "Next Track",
        },
        MediaTrackPrevious => {
            serialized_suffix: "media.previous.track",
            translation_id: Some(StringId::new("key.keyboard.media.previous.track")),
            fallback: "Previous Track",
        },
        Power => {
            serialized_suffix: "power",
            translation_id: Some(StringId::new("key.keyboard.power")),
            fallback: "Power",
        },
        Sleep => {
            serialized_suffix: "sleep",
            translation_id: Some(StringId::new("key.keyboard.sleep")),
            fallback: "Sleep",
        },
        AudioVolumeDown => {
            serialized_suffix: "volume.down",
            translation_id: Some(StringId::new("key.keyboard.volume.down")),
            fallback: "Volume Down",
        },
        AudioVolumeMute => {
            serialized_suffix: "mute",
            translation_id: Some(StringId::new("key.keyboard.mute")),
            fallback: "Mute",
        },
        AudioVolumeUp => {
            serialized_suffix: "volume.up",
            translation_id: Some(StringId::new("key.keyboard.volume.up")),
            fallback: "Volume Up",
        },
        WakeUp => {
            serialized_suffix: "wake",
            translation_id: Some(StringId::new("key.keyboard.wake")),
            fallback: "Wake",
        },
        Meta => {
            serialized_suffix: "meta",
            translation_id: None,
            fallback: "Meta",
        },
        Hyper => {
            serialized_suffix: "hyper",
            translation_id: None,
            fallback: "Hyper",
        },
        Turbo => {
            serialized_suffix: "turbo",
            translation_id: None,
            fallback: "Turbo",
        },
        Abort => {
            serialized_suffix: "abort",
            translation_id: None,
            fallback: "Abort",
        },
        Resume => {
            serialized_suffix: "resume",
            translation_id: None,
            fallback: "Resume",
        },
        Suspend => {
            serialized_suffix: "suspend",
            translation_id: None,
            fallback: "Suspend",
        },
        Again => {
            serialized_suffix: "again",
            translation_id: Some(StringId::new("key.keyboard.again")),
            fallback: "Again",
        },
        Copy => {
            serialized_suffix: "copy",
            translation_id: Some(StringId::new("key.keyboard.copy")),
            fallback: "Copy",
        },
        Cut => {
            serialized_suffix: "cut",
            translation_id: Some(StringId::new("key.keyboard.cut")),
            fallback: "Cut",
        },
        Find => {
            serialized_suffix: "find",
            translation_id: Some(StringId::new("key.keyboard.find")),
            fallback: "Find",
        },
        Open => {
            serialized_suffix: "open",
            translation_id: None,
            fallback: "Open",
        },
        Paste => {
            serialized_suffix: "paste",
            translation_id: Some(StringId::new("key.keyboard.paste")),
            fallback: "Paste",
        },
        Props => {
            serialized_suffix: "props",
            translation_id: None,
            fallback: "Props",
        },
        Select => {
            serialized_suffix: "select",
            translation_id: Some(StringId::new("key.keyboard.select")),
            fallback: "Select",
        },
        Undo => {
            serialized_suffix: "undo",
            translation_id: Some(StringId::new("key.keyboard.undo")),
            fallback: "Undo",
        },
        Hiragana => {
            serialized_suffix: "hiragana",
            translation_id: None,
            fallback: "Hiragana",
        },
        Katakana => {
            serialized_suffix: "katakana",
            translation_id: None,
            fallback: "Katakana",
        },
        F1 => {
            serialized_suffix: "f1",
            translation_id: Some(StringId::new("key.keyboard.f1")),
            fallback: "F1",
        },
        F2 => {
            serialized_suffix: "f2",
            translation_id: Some(StringId::new("key.keyboard.f2")),
            fallback: "F2",
        },
        F3 => {
            serialized_suffix: "f3",
            translation_id: Some(StringId::new("key.keyboard.f3")),
            fallback: "F3",
        },
        F4 => {
            serialized_suffix: "f4",
            translation_id: Some(StringId::new("key.keyboard.f4")),
            fallback: "F4",
        },
        F5 => {
            serialized_suffix: "f5",
            translation_id: Some(StringId::new("key.keyboard.f5")),
            fallback: "F5",
        },
        F6 => {
            serialized_suffix: "f6",
            translation_id: Some(StringId::new("key.keyboard.f6")),
            fallback: "F6",
        },
        F7 => {
            serialized_suffix: "f7",
            translation_id: Some(StringId::new("key.keyboard.f7")),
            fallback: "F7",
        },
        F8 => {
            serialized_suffix: "f8",
            translation_id: Some(StringId::new("key.keyboard.f8")),
            fallback: "F8",
        },
        F9 => {
            serialized_suffix: "f9",
            translation_id: Some(StringId::new("key.keyboard.f9")),
            fallback: "F9",
        },
        F10 => {
            serialized_suffix: "f10",
            translation_id: Some(StringId::new("key.keyboard.f10")),
            fallback: "F10",
        },
        F11 => {
            serialized_suffix: "f11",
            translation_id: Some(StringId::new("key.keyboard.f11")),
            fallback: "F11",
        },
        F12 => {
            serialized_suffix: "f12",
            translation_id: Some(StringId::new("key.keyboard.f12")),
            fallback: "F12",
        },
        F13 => {
            serialized_suffix: "f13",
            translation_id: Some(StringId::new("key.keyboard.f13")),
            fallback: "F13",
        },
        F14 => {
            serialized_suffix: "f14",
            translation_id: Some(StringId::new("key.keyboard.f14")),
            fallback: "F14",
        },
        F15 => {
            serialized_suffix: "f15",
            translation_id: Some(StringId::new("key.keyboard.f15")),
            fallback: "F15",
        },
        F16 => {
            serialized_suffix: "f16",
            translation_id: Some(StringId::new("key.keyboard.f16")),
            fallback: "F16",
        },
        F17 => {
            serialized_suffix: "f17",
            translation_id: Some(StringId::new("key.keyboard.f17")),
            fallback: "F17",
        },
        F18 => {
            serialized_suffix: "f18",
            translation_id: Some(StringId::new("key.keyboard.f18")),
            fallback: "F18",
        },
        F19 => {
            serialized_suffix: "f19",
            translation_id: Some(StringId::new("key.keyboard.f19")),
            fallback: "F19",
        },
        F20 => {
            serialized_suffix: "f20",
            translation_id: Some(StringId::new("key.keyboard.f20")),
            fallback: "F20",
        },
        F21 => {
            serialized_suffix: "f21",
            translation_id: Some(StringId::new("key.keyboard.f21")),
            fallback: "F21",
        },
        F22 => {
            serialized_suffix: "f22",
            translation_id: Some(StringId::new("key.keyboard.f22")),
            fallback: "F22",
        },
        F23 => {
            serialized_suffix: "f23",
            translation_id: Some(StringId::new("key.keyboard.f23")),
            fallback: "F23",
        },
        F24 => {
            serialized_suffix: "f24",
            translation_id: Some(StringId::new("key.keyboard.f24")),
            fallback: "F24",
        },
        F25 => {
            serialized_suffix: "f25",
            translation_id: Some(StringId::new("key.keyboard.f25")),
            fallback: "F25",
        },
        F26 => {
            serialized_suffix: "f26",
            translation_id: None,
            fallback: "F26",
        },
        F27 => {
            serialized_suffix: "f27",
            translation_id: None,
            fallback: "F27",
        },
        F28 => {
            serialized_suffix: "f28",
            translation_id: None,
            fallback: "F28",
        },
        F29 => {
            serialized_suffix: "f29",
            translation_id: None,
            fallback: "F29",
        },
        F30 => {
            serialized_suffix: "f30",
            translation_id: None,
            fallback: "F30",
        },
        F31 => {
            serialized_suffix: "f31",
            translation_id: None,
            fallback: "F31",
        },
        F32 => {
            serialized_suffix: "f32",
            translation_id: None,
            fallback: "F32",
        },
        F33 => {
            serialized_suffix: "f33",
            translation_id: None,
            fallback: "F33",
        },
        F34 => {
            serialized_suffix: "f34",
            translation_id: None,
            fallback: "F34",
        },
        F35 => {
            serialized_suffix: "f35",
            translation_id: None,
            fallback: "F35",
        },
    }
}
