use anyhow::{Context, Result, anyhow, bail};
use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    style::Color,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub keys: KeyConfig,
    pub mouse: MouseConfig,
    pub behavior: BehaviorConfig,
    pub defaults: DefaultsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub status_position: StatusPosition,
    pub show_pane_labels: bool,
    pub status_clock: bool,
    pub status_show_pane: bool,
    pub status_show_window_list: bool,
    pub status_style: StatusStyle,
    pub status: StatusConfig,
    pub dividers: DividerConfig,
    pub theme: ThemeConfig,
    pub chooser: OverlayConfig,
    pub help: OverlayConfig,
    #[serde(rename = "copy_mode")]
    pub copy_mode: ModeBarConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusConfig {
    pub show_sessions: bool,
    pub show_window_list: bool,
    pub show_host: bool,
    pub show_clock: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DividerConfig {
    pub charset: DividerCharset,
    pub highlight_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    pub border: bool,
    pub title: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeBarConfig {
    pub show_hints: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub status: StyleConfig,
    pub current_session: StyleConfig,
    pub other_session: StyleConfig,
    pub active_window: StyleConfig,
    pub inactive_window: StyleConfig,
    pub last_window: StyleConfig,
    pub right_status: StyleConfig,
    pub message: StyleConfig,
    pub prompt: StyleConfig,
    pub copy_mode: StyleConfig,
    pub divider: StyleConfig,
    pub active_divider: StyleConfig,
    pub selection: StyleConfig,
    pub chooser_selected: StyleConfig,
    pub chooser_border: StyleConfig,
    pub help: StyleConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StyleConfig {
    pub fg: Option<ThemeColor>,
    pub bg: Option<ThemeColor>,
    pub bold: bool,
    pub dim: bool,
    pub reverse: bool,
    pub underline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyConfig {
    pub prefix: String,
    pub bindings: BTreeMap<String, String>,
    pub normal: BTreeMap<String, String>,
    pub leader: BTreeMap<String, String>,
    pub copy_mode: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
struct RawKeyConfig {
    pub prefix: Option<String>,
    pub bindings: BTreeMap<String, String>,
    pub normal: BTreeMap<String, String>,
    pub leader: Option<toml::Value>,
    #[serde(rename = "copy_mode")]
    pub copy_mode: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseConfig {
    pub enabled: bool,
    pub focus_on_click: bool,
    pub selection_copy: bool,
    pub border_resize: bool,
    pub wheel_scroll: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    pub scrollback_lines: usize,
    pub default_shell: Option<String>,
    pub resize_step: u16,
    pub copy_page_size: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultsConfig {
    pub session: SessionDefaults,
    pub window: WindowDefaults,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionDefaults {
    pub name_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowDefaults {
    pub shell_name: String,
    pub use_command_name: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StatusPosition {
    Top,
    #[default]
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StatusStyle {
    #[default]
    TmuxPlus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DividerCharset {
    #[default]
    Unicode,
    Ascii,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeColor {
    Reset,
    Black,
    DarkGrey,
    Red,
    DarkRed,
    Green,
    DarkGreen,
    Yellow,
    DarkYellow,
    Blue,
    DarkBlue,
    Magenta,
    DarkMagenta,
    Cyan,
    DarkCyan,
    White,
    Grey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub ui: ResolvedUiConfig,
    pub keys: ResolvedKeyConfig,
    pub mouse: MouseConfig,
    pub behavior: BehaviorConfig,
    pub defaults: DefaultsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUiConfig {
    pub status_position: StatusPosition,
    pub show_pane_labels: bool,
    pub status: StatusConfig,
    pub dividers: DividerConfig,
    pub theme: ThemeConfig,
    pub chooser: OverlayConfig,
    pub help: OverlayConfig,
    pub copy_mode: ModeBarConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeyConfig {
    pub prefix: KeyPattern,
    pub normal: KeyTable,
    pub leader: KeyTable,
    pub copy_mode: KeyTable,
}

impl Default for ResolvedKeyConfig {
    fn default() -> Self {
        Config::default()
            .resolve()
            .expect("default config should resolve")
            .keys
    }
}

pub type KeyTable = Vec<(KeyPattern, Action)>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyPattern {
    pub code: KeyPatternCode,
    pub modifiers: KeyPatternModifiers,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyPatternCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Delete,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct KeyPatternModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Detach,
    SplitVertical,
    SplitHorizontal,
    OpenPrompt,
    OpenSessions,
    OpenHelp,
    NewWindow,
    NextWindow,
    PrevWindow,
    SelectWindowIndex(u8),
    FocusLeft,
    FocusDown,
    FocusUp,
    FocusRight,
    ResizeLeft,
    ResizeDown,
    ResizeUp,
    ResizeRight,
    KillPane,
    PasteTopBuffer,
    ListBuffers,
    DeleteTopBuffer,
    ChooseBuffer,
    EnterCopyMode,
    ExitCopyMode,
    CopyMoveLeft,
    CopyMoveDown,
    CopyMoveUp,
    CopyMoveRight,
    CopyLineStart,
    CopyLineEnd,
    CopyTop,
    CopyBottom,
    CopyPageUp,
    CopyPageDown,
    CopyStartSelection,
    CopyYank,
    ReloadConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ui: UiConfig::default(),
            keys: KeyConfig::default(),
            mouse: MouseConfig::default(),
            behavior: BehaviorConfig::default(),
            defaults: DefaultsConfig::default(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            status_position: StatusPosition::Bottom,
            show_pane_labels: true,
            status_clock: true,
            status_show_pane: true,
            status_show_window_list: true,
            status_style: StatusStyle::TmuxPlus,
            status: StatusConfig::default(),
            dividers: DividerConfig::default(),
            theme: ThemeConfig::default(),
            chooser: OverlayConfig::default(),
            help: OverlayConfig::default(),
            copy_mode: ModeBarConfig::default(),
        }
    }
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            show_sessions: true,
            show_window_list: true,
            show_host: true,
            show_clock: true,
        }
    }
}

impl Default for DividerConfig {
    fn default() -> Self {
        Self {
            charset: DividerCharset::Unicode,
            highlight_active: true,
        }
    }
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            border: true,
            title: true,
        }
    }
}

impl Default for ModeBarConfig {
    fn default() -> Self {
        Self { show_hints: true }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            status: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            current_session: StyleConfig {
                reverse: true,
                bold: true,
                ..StyleConfig::default()
            },
            other_session: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            active_window: StyleConfig {
                reverse: true,
                bold: true,
                ..StyleConfig::default()
            },
            inactive_window: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            last_window: StyleConfig {
                reverse: true,
                dim: true,
                ..StyleConfig::default()
            },
            right_status: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            message: StyleConfig {
                reverse: true,
                bold: true,
                ..StyleConfig::default()
            },
            prompt: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            copy_mode: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            divider: StyleConfig::default(),
            active_divider: StyleConfig {
                bold: true,
                ..StyleConfig::default()
            },
            selection: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            chooser_selected: StyleConfig {
                reverse: true,
                ..StyleConfig::default()
            },
            chooser_border: StyleConfig {
                dim: true,
                ..StyleConfig::default()
            },
            help: StyleConfig::default(),
        }
    }
}

impl Default for KeyConfig {
    fn default() -> Self {
        Self {
            prefix: "Ctrl-b".into(),
            bindings: default_legacy_leader_bindings(),
            normal: BTreeMap::new(),
            leader: default_leader_bindings(),
            copy_mode: default_copy_mode_bindings(),
        }
    }
}

impl Default for RawKeyConfig {
    fn default() -> Self {
        Self {
            prefix: None,
            bindings: BTreeMap::new(),
            normal: BTreeMap::new(),
            leader: None,
            copy_mode: BTreeMap::new(),
        }
    }
}

impl<'de> Deserialize<'de> for KeyConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawKeyConfig::deserialize(deserializer)?;
        let defaults = KeyConfig::default();
        let mut config = KeyConfig {
            prefix: raw.prefix.unwrap_or_else(|| defaults.prefix.clone()),
            bindings: raw.bindings,
            normal: raw.normal,
            leader: BTreeMap::new(),
            copy_mode: raw.copy_mode,
        };

        if let Some(value) = raw.leader {
            match value {
                toml::Value::String(prefix) => config.prefix = prefix,
                toml::Value::Table(table) => {
                    config.leader = table
                        .into_iter()
                        .map(|(key, value)| match value {
                            toml::Value::String(text) => Ok((key, text)),
                            _ => Err(serde::de::Error::custom(
                                "keys.leader bindings must be string values",
                            )),
                        })
                        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
                }
                _ => {
                    return Err(serde::de::Error::custom(
                        "keys.leader must be either a prefix string or a table of bindings",
                    ));
                }
            }
        }

        Ok(config)
    }
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            focus_on_click: true,
            selection_copy: true,
            border_resize: true,
            wheel_scroll: true,
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            default_shell: None,
            resize_step: 50,
            copy_page_size: None,
        }
    }
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            session: SessionDefaults::default(),
            window: WindowDefaults::default(),
        }
    }
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self {
            name_prefix: "session".into(),
        }
    }
}

impl Default for WindowDefaults {
    fn default() -> Self {
        Self {
            shell_name: "shell".into(),
            use_command_name: true,
        }
    }
}

impl Config {
    pub fn from_toml(input: &str) -> Result<Self> {
        let config = toml::from_str(input).context("failed to parse config TOML")?;
        Ok(config)
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file at {}", path.display()))?;
        Self::from_toml(&raw)
    }

    pub fn resolve(&self) -> Result<ResolvedConfig> {
        let status = resolve_status_config(&self.ui);
        let key_config = resolve_key_config(&self.keys, self.behavior.resize_step)?;
        Ok(ResolvedConfig {
            ui: ResolvedUiConfig {
                status_position: self.ui.status_position,
                show_pane_labels: self.ui.show_pane_labels,
                status,
                dividers: self.ui.dividers.clone(),
                theme: self.ui.theme.clone(),
                chooser: self.ui.chooser.clone(),
                help: self.ui.help.clone(),
                copy_mode: self.ui.copy_mode.clone(),
            },
            keys: key_config,
            mouse: self.mouse.clone(),
            behavior: self.behavior.clone(),
            defaults: self.defaults.clone(),
        })
    }
}

fn resolve_status_config(ui: &UiConfig) -> StatusConfig {
    let mut status = ui.status.clone();
    if ui.status_clock != UiConfig::default().status_clock {
        status.show_clock = ui.status_clock;
    }
    if ui.status_show_window_list != UiConfig::default().status_show_window_list {
        status.show_window_list = ui.status_show_window_list;
    }
    status
}

fn resolve_key_config(config: &KeyConfig, resize_step: u16) -> Result<ResolvedKeyConfig> {
    let prefix = parse_key_pattern(&config.prefix)
        .with_context(|| format!("invalid keys.prefix '{}'", config.prefix))?;
    let normal = resolve_table("keys.normal", &config.normal, resize_step)?;

    let mut leader_raw = default_leader_bindings();
    for (action, key) in &config.bindings {
        leader_raw.insert(action.clone(), key.clone());
    }
    for (action, key) in &config.leader {
        leader_raw.insert(action.clone(), key.clone());
    }
    let leader = resolve_table("keys.leader", &leader_raw, resize_step)?;
    let copy_mode = resolve_table("keys.copy_mode", &config.copy_mode, resize_step)?;

    Ok(ResolvedKeyConfig {
        prefix,
        normal,
        leader,
        copy_mode,
    })
}

fn resolve_table(
    section: &str,
    values: &BTreeMap<String, String>,
    _resize_step: u16,
) -> Result<KeyTable> {
    let mut table = BTreeMap::<KeyPattern, Action>::new();
    for (action_name, key_name) in values {
        let action = parse_action_name(action_name)
            .with_context(|| format!("invalid action '{action_name}' in {section}"))?;
        let pattern = parse_key_pattern(key_name).with_context(|| {
            format!("invalid key '{key_name}' for action '{action_name}' in {section}")
        })?;
        if let Some(existing) = table.insert(pattern.clone(), action) {
            bail!(
                "duplicate binding '{}' in {section}; conflicts with {:?}",
                key_name,
                existing
            );
        }
    }
    Ok(table.into_iter().collect())
}

pub fn parse_key_pattern(value: &str) -> Result<KeyPattern> {
    if value == "-" {
        return Ok(KeyPattern {
            code: KeyPatternCode::Char('-'),
            modifiers: KeyPatternModifiers::default(),
        });
    }
    let mut modifiers = KeyPatternModifiers::default();
    let mut parts = value.split('-').peekable();
    let mut last = None;
    while let Some(part) = parts.next() {
        if parts.peek().is_some() {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.control = true,
                "alt" | "meta" => modifiers.alt = true,
                "shift" => modifiers.shift = true,
                other => return Err(anyhow!("unknown modifier '{other}'")),
            }
        } else {
            last = Some(part);
        }
    }
    let key = last.ok_or_else(|| anyhow!("missing key code"))?;
    let lower = key.to_ascii_lowercase();
    let code = match lower.as_str() {
        "enter" => KeyPatternCode::Enter,
        "esc" | "escape" => KeyPatternCode::Esc,
        "tab" => KeyPatternCode::Tab,
        "space" => KeyPatternCode::Char(' '),
        "backspace" => KeyPatternCode::Backspace,
        "left" => KeyPatternCode::Left,
        "right" => KeyPatternCode::Right,
        "up" => KeyPatternCode::Up,
        "down" => KeyPatternCode::Down,
        "home" => KeyPatternCode::Home,
        "end" => KeyPatternCode::End,
        "delete" | "del" => KeyPatternCode::Delete,
        "pageup" | "page-up" => KeyPatternCode::PageUp,
        "pagedown" | "page-down" => KeyPatternCode::PageDown,
        _ if key.chars().count() == 1 => KeyPatternCode::Char(key.chars().next().unwrap()),
        _ => return Err(anyhow!("unknown key '{key}'")),
    };
    Ok(KeyPattern { code, modifiers })
}

pub fn key_event_matches(pattern: &KeyPattern, event: KeyEvent) -> bool {
    if pattern.code != key_event_code(&event) {
        return false;
    }
    if pattern.modifiers.control != event.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    if pattern.modifiers.alt != event.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    if matches!(pattern.code, KeyPatternCode::Char(_)) {
        return true;
    }
    pattern.modifiers.shift == event.modifiers.contains(KeyModifiers::SHIFT)
}

fn key_event_code(event: &KeyEvent) -> KeyPatternCode {
    match event.code {
        KeyCode::Char(ch) => KeyPatternCode::Char(ch),
        KeyCode::Enter => KeyPatternCode::Enter,
        KeyCode::Esc => KeyPatternCode::Esc,
        KeyCode::Tab => KeyPatternCode::Tab,
        KeyCode::Backspace => KeyPatternCode::Backspace,
        KeyCode::Left => KeyPatternCode::Left,
        KeyCode::Right => KeyPatternCode::Right,
        KeyCode::Up => KeyPatternCode::Up,
        KeyCode::Down => KeyPatternCode::Down,
        KeyCode::Home => KeyPatternCode::Home,
        KeyCode::End => KeyPatternCode::End,
        KeyCode::Delete => KeyPatternCode::Delete,
        KeyCode::PageUp => KeyPatternCode::PageUp,
        KeyCode::PageDown => KeyPatternCode::PageDown,
        _ => KeyPatternCode::Char('\0'),
    }
}

fn parse_action_name(value: &str) -> Result<Action> {
    Ok(match value {
        "detach" => Action::Detach,
        "split_vertical" | "split-right" => Action::SplitVertical,
        "split_horizontal" | "split-down" => Action::SplitHorizontal,
        "open_prompt" => Action::OpenPrompt,
        "open_sessions" => Action::OpenSessions,
        "open_help" => Action::OpenHelp,
        "new_window" => Action::NewWindow,
        "next_window" => Action::NextWindow,
        "prev_window" | "previous_window" => Action::PrevWindow,
        "select_window_0" => Action::SelectWindowIndex(0),
        "select_window_1" => Action::SelectWindowIndex(1),
        "select_window_2" => Action::SelectWindowIndex(2),
        "select_window_3" => Action::SelectWindowIndex(3),
        "select_window_4" => Action::SelectWindowIndex(4),
        "select_window_5" => Action::SelectWindowIndex(5),
        "select_window_6" => Action::SelectWindowIndex(6),
        "select_window_7" => Action::SelectWindowIndex(7),
        "select_window_8" => Action::SelectWindowIndex(8),
        "select_window_9" => Action::SelectWindowIndex(9),
        "focus_left" => Action::FocusLeft,
        "focus_down" => Action::FocusDown,
        "focus_up" => Action::FocusUp,
        "focus_right" => Action::FocusRight,
        "resize_left" => Action::ResizeLeft,
        "resize_down" => Action::ResizeDown,
        "resize_up" => Action::ResizeUp,
        "resize_right" => Action::ResizeRight,
        "kill_pane" => Action::KillPane,
        "paste_top_buffer" => Action::PasteTopBuffer,
        "list_buffers" => Action::ListBuffers,
        "delete_top_buffer" => Action::DeleteTopBuffer,
        "choose_buffer" => Action::ChooseBuffer,
        "enter_copy_mode" => Action::EnterCopyMode,
        "exit_copy_mode" => Action::ExitCopyMode,
        "copy_move_left" => Action::CopyMoveLeft,
        "copy_move_down" => Action::CopyMoveDown,
        "copy_move_up" => Action::CopyMoveUp,
        "copy_move_right" => Action::CopyMoveRight,
        "copy_line_start" => Action::CopyLineStart,
        "copy_line_end" => Action::CopyLineEnd,
        "copy_top" => Action::CopyTop,
        "copy_bottom" => Action::CopyBottom,
        "copy_page_up" => Action::CopyPageUp,
        "copy_page_down" => Action::CopyPageDown,
        "copy_start_selection" => Action::CopyStartSelection,
        "copy_yank" => Action::CopyYank,
        "reload_config" => Action::ReloadConfig,
        other => return Err(anyhow!("unknown action '{other}'")),
    })
}

fn default_legacy_leader_bindings() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn default_leader_bindings() -> BTreeMap<String, String> {
    let mut bindings = BTreeMap::new();
    bindings.insert("detach".into(), "d".into());
    bindings.insert("split_vertical".into(), "%".into());
    bindings.insert("split_horizontal".into(), "\"".into());
    bindings.insert("open_prompt".into(), ":".into());
    bindings.insert("enter_copy_mode".into(), "[".into());
    bindings.insert("open_sessions".into(), "s".into());
    bindings.insert("open_help".into(), "?".into());
    bindings.insert("new_window".into(), "c".into());
    bindings.insert("next_window".into(), "n".into());
    bindings.insert("prev_window".into(), "p".into());
    bindings.insert("focus_left".into(), "h".into());
    bindings.insert("focus_down".into(), "j".into());
    bindings.insert("focus_up".into(), "k".into());
    bindings.insert("focus_right".into(), "l".into());
    bindings.insert("resize_left".into(), "H".into());
    bindings.insert("resize_down".into(), "J".into());
    bindings.insert("resize_up".into(), "K".into());
    bindings.insert("resize_right".into(), "L".into());
    bindings.insert("kill_pane".into(), "x".into());
    bindings.insert("paste_top_buffer".into(), "]".into());
    bindings.insert("list_buffers".into(), "#".into());
    bindings.insert("delete_top_buffer".into(), "-".into());
    bindings.insert("choose_buffer".into(), "=".into());
    bindings.insert("select_window_0".into(), "0".into());
    bindings.insert("select_window_1".into(), "1".into());
    bindings.insert("select_window_2".into(), "2".into());
    bindings.insert("select_window_3".into(), "3".into());
    bindings.insert("select_window_4".into(), "4".into());
    bindings.insert("select_window_5".into(), "5".into());
    bindings.insert("select_window_6".into(), "6".into());
    bindings.insert("select_window_7".into(), "7".into());
    bindings.insert("select_window_8".into(), "8".into());
    bindings.insert("select_window_9".into(), "9".into());
    bindings.insert("reload_config".into(), "r".into());
    bindings
}

fn default_copy_mode_bindings() -> BTreeMap<String, String> {
    let mut bindings = BTreeMap::new();
    bindings.insert("exit_copy_mode".into(), "Esc".into());
    bindings.insert("copy_move_left".into(), "h".into());
    bindings.insert("copy_move_right".into(), "l".into());
    bindings.insert("copy_move_up".into(), "k".into());
    bindings.insert("copy_move_down".into(), "j".into());
    bindings.insert("copy_line_start".into(), "0".into());
    bindings.insert("copy_line_end".into(), "$".into());
    bindings.insert("copy_top".into(), "g".into());
    bindings.insert("copy_bottom".into(), "G".into());
    bindings.insert("copy_page_up".into(), "PageUp".into());
    bindings.insert("copy_page_down".into(), "PageDown".into());
    bindings.insert("copy_start_selection".into(), "Space".into());
    bindings.insert("copy_yank".into(), "y".into());
    bindings
}

impl ThemeColor {
    pub fn to_crossterm(self) -> Color {
        match self {
            ThemeColor::Reset => Color::Reset,
            ThemeColor::Black => Color::Black,
            ThemeColor::DarkGrey => Color::DarkGrey,
            ThemeColor::Red => Color::Red,
            ThemeColor::DarkRed => Color::DarkRed,
            ThemeColor::Green => Color::Green,
            ThemeColor::DarkGreen => Color::DarkGreen,
            ThemeColor::Yellow => Color::Yellow,
            ThemeColor::DarkYellow => Color::DarkYellow,
            ThemeColor::Blue => Color::Blue,
            ThemeColor::DarkBlue => Color::DarkBlue,
            ThemeColor::Magenta => Color::Magenta,
            ThemeColor::DarkMagenta => Color::DarkMagenta,
            ThemeColor::Cyan => Color::Cyan,
            ThemeColor::DarkCyan => Color::DarkCyan,
            ThemeColor::White => Color::White,
            ThemeColor::Grey => Color::Grey,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn empty_config_uses_defaults() {
        let config = Config::from_toml("").expect("default config");
        let resolved = config.resolve().expect("resolve config");
        assert_eq!(config.keys.prefix, "Ctrl-b");
        assert_eq!(resolved.ui.status_position, StatusPosition::Bottom);
        assert!(resolved.ui.status.show_clock);
        assert!(resolved.ui.status.show_window_list);
        assert_eq!(resolved.behavior.scrollback_lines, 10_000);
        assert!(resolved.mouse.enabled);
        assert!(
            resolved
                .keys
                .leader
                .iter()
                .any(|(_, action)| *action == Action::Detach)
        );
    }

    #[test]
    fn partial_config_overrides_defaults() {
        let config = Config::from_toml(
            r#"
                [ui]
                status_position = "top"

                [behavior]
                scrollback_lines = 2048

                [keys.leader]
                new_window = "w"
            "#,
        )
        .expect("partial config");
        let resolved = config.resolve().expect("resolve config");
        assert_eq!(resolved.ui.status_position, StatusPosition::Top);
        assert_eq!(resolved.behavior.scrollback_lines, 2048);
        assert!(resolved.keys.leader.iter().any(|(pattern, action)| {
            *action == Action::NewWindow && *pattern == parse_key_pattern("w").expect("pattern")
        }));
    }

    #[test]
    fn defaults_include_buffer_bindings() {
        let resolved = Config::default().resolve().expect("resolve config");
        assert!(
            resolved
                .keys
                .leader
                .iter()
                .any(|(_, action)| *action == Action::PasteTopBuffer)
        );
        assert!(
            resolved
                .keys
                .leader
                .iter()
                .any(|(_, action)| *action == Action::ChooseBuffer)
        );
    }

    #[test]
    fn loading_config_from_path_reads_toml() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("config.toml");
        fs::write(&path, "[mouse]\nenabled = false\n").expect("write config");

        let config = Config::load_from_path(&path).expect("load config");

        assert!(!config.mouse.enabled);
    }

    #[test]
    fn legacy_leader_alias_still_parses() {
        let config = Config::from_toml(
            r#"
                [keys]
                leader = "Ctrl-a"
                [keys.bindings]
                detach = "q"
            "#,
        )
        .expect("legacy config");
        let resolved = config.resolve().expect("resolve");
        assert_eq!(
            resolved.keys.prefix,
            parse_key_pattern("Ctrl-a").expect("prefix")
        );
        assert!(
            resolved
                .keys
                .leader
                .iter()
                .any(|(pattern, action)| *action == Action::Detach
                    && *pattern == parse_key_pattern("q").expect("pattern"))
        );
    }

    #[test]
    fn duplicate_binding_in_same_mode_is_rejected() {
        let config = Config::from_toml(
            r#"
                [keys.leader]
                detach = "d"
                new_window = "d"
            "#,
        )
        .expect("config");
        let error = config.resolve().expect_err("duplicate binding");
        assert!(error.to_string().contains("duplicate binding"));
    }

    #[test]
    fn invalid_key_name_is_rejected() {
        let config = Config::from_toml(
            r#"
                [keys.leader]
                detach = "Ctrl-Magic"
            "#,
        )
        .expect("config");
        let error = config.resolve().expect_err("invalid key");
        assert!(error.to_string().contains("invalid key"));
    }

    #[test]
    fn invalid_action_is_rejected() {
        let config = Config::from_toml(
            r#"
                [keys.leader]
                magic = "m"
            "#,
        )
        .expect("config");
        let error = config.resolve().expect_err("invalid action");
        assert!(error.to_string().contains("invalid action"));
    }

    #[test]
    fn key_event_matching_recognizes_control_keys() {
        let pattern = parse_key_pattern("Ctrl-b").expect("pattern");
        assert!(key_event_matches(
            &pattern,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)
        ));
    }

    #[test]
    fn theme_color_maps_to_crossterm() {
        assert_eq!(ThemeColor::DarkBlue.to_crossterm(), Color::DarkBlue);
    }
}
