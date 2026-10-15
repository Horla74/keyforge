//! The remapping state machine.
//!
//! The engine is deliberately platform independent and fully synchronous: the
//! backend feeds it physical key transitions plus a clock, and gets back the
//! decision "swallow this?" together with the events that must be injected.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::{Action, CompiledConfig, CompiledMap, ComboMode};
use crate::keycode::{KeyCode, KeyMatcher};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyState {
    Down,
    Up,
}

/// Why the operating system currently thinks a key is held.
///
/// This distinction is what keeps remapping free of duplicates:
/// * `Physical` — the OS received the real key press (the backend did *not*
///   swallow it), so the OS will also deliver the matching release itself and
///   the engine must not inject anything for that key.
/// * `Injected` — the engine produced the press, so it owes the release.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DownSource {
    Physical,
    Injected,
}

/// Something the backend has to do after the engine decided.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Output {
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    Command(String),
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// Do not deliver the physical event to the operating system.
    pub swallow: bool,
    pub outputs: Vec<Output>,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub key_events: u64,
    pub activations: u64,
    pub commands: u64,
}

#[derive(Debug)]
struct Active {
    profile: String,
    trigger: Vec<KeyMatcher>,
    /// Keys this mapping has pressed and must release again.
    outputs: Vec<KeyCode>,
    /// Modifiers that were already delivered and had to be released so the
    /// combination does not leak into the output.
    restore: Vec<KeyCode>,
    /// Momentary layer pushed by this mapping, with the key holding it.
    layer: Option<(String, KeyCode)>,
}

#[derive(Debug)]
struct Pending {
    key: KeyCode,
    profile: String,
    trigger: Vec<KeyMatcher>,
    tap: Vec<KeyCode>,
    hold: Vec<KeyCode>,
    deadline: Instant,
}

pub struct Engine {
    cfg: CompiledConfig,
    /// Physical keys currently held, in press order.
    pressed: Vec<KeyCode>,
    /// Keys the OS currently has down, and who put them there.
    emitted: HashMap<KeyCode, DownSource>,
    /// Does the backend drop non-swallowed events (Linux evdev grab), so the
    /// engine has to re-inject them? Windows/macOS hooks let the OS deliver
    /// them, and re-injecting would duplicate every keystroke.
    reinject_passthrough: bool,
    /// Hold Win back while it might be part of a `meta + ...` mapping.
    defer_meta: bool,
    /// The Win press currently held back, if any.
    deferred_meta: Option<DeferredMeta>,
    active: Vec<Active>,
    momentary: Vec<String>,
    toggled: Vec<String>,
    /// Keys that are held purely to keep a momentary layer open. They stay
    /// physically down but are excluded from combo matching, otherwise a layer
    /// mapping could never match (`f1` + `h` would look like a 2 key combo).
    layer_sources: Vec<KeyCode>,
    pending: Option<Pending>,
    front_app: Option<String>,
    /// `"<origin>: <trigger> -> <action>"` of the most recent activation.
    last_activation: Option<String>,
    pub stats: Stats,
}

impl Engine {
    pub fn new(cfg: CompiledConfig) -> Engine {
        let defer_meta = cfg.uses_meta_trigger();
        Engine {
            cfg,
            pressed: Vec::new(),
            emitted: HashMap::new(),
            reinject_passthrough: false,
            defer_meta,
            deferred_meta: None,
            active: Vec::new(),
            momentary: Vec::new(),
            toggled: Vec::new(),
            layer_sources: Vec::new(),
            pending: None,
            front_app: None,
            last_activation: None,
            stats: Stats::default(),
        }
    }

    /// Mapping description of the most recent activation, for logs and tests.
    pub fn take_last_activation(&mut self) -> Option<String> {
        self.last_activation.take()
    }

    /// Release every key we injected and forget all transient state.
    ///
    /// Called when the daemon stops: a mapping that is held while keyforge
    /// exits would otherwise leave its output key pressed forever (the classic
    /// "ctrl is stuck after I killed the remapper" problem).
    pub fn release_all(&mut self) -> Vec<Output> {
        let mut outputs = Vec::new();
        let injected: Vec<KeyCode> = self
            .emitted
            .iter()
            .filter(|(_, source)| **source == DownSource::Injected)
            .map(|(key, _)| *key)
            .collect();
        for key in injected {
            self.release_owned(key, &mut outputs);
        }
        self.emitted.clear();
        self.active.clear();
        self.momentary.clear();
        self.toggled.clear();
        self.layer_sources.clear();
        self.pending = None;
        self.deferred_meta = None;
        self.pressed.clear();
        outputs
    }

    /// Tell the engine whether the backend needs non-swallowed events to be
    /// re-injected.
    ///
    /// * `false` (default, Windows/macOS hooks): the OS delivers everything the
    ///   hook passes on, so the engine only injects what it swallowed.
    /// * `true` (Linux evdev grab): the backend suppresses the original event,
    ///   so the engine must re-inject the ones it decided not to swallow.
    pub fn set_reinject_passthrough(&mut self, on: bool) {
        self.reinject_passthrough = on;
    }

    pub fn reinjects_passthrough(&self) -> bool {
        self.reinject_passthrough
    }

    /// Inject a press unless the OS already has that key down.
    fn press_once(&mut self, key: KeyCode, outputs: &mut Vec<Output>) -> bool {
        if self.emitted.contains_key(&key) {
            return false;
        }
        self.emitted.insert(key, DownSource::Injected);
        outputs.push(Output::KeyDown(key));
        true
    }

    /// Release a key, but only one we injected: a key the OS delivered itself
    /// is released by the OS's own key-up event.
    fn release_owned(&mut self, key: KeyCode, outputs: &mut Vec<Output>) -> bool {
        match self.emitted.remove(&key) {
            Some(DownSource::Injected) => {
                outputs.push(Output::KeyUp(key));
                true
            }
            Some(DownSource::Physical) => {
                self.emitted.insert(key, DownSource::Physical);
                false
            }
            None => false,
        }
    }

    pub fn config(&self) -> &CompiledConfig {
        &self.cfg
    }

    pub fn pressed(&self) -> &[KeyCode] {
        &self.pressed
    }

    pub fn layers(&self) -> (Vec<&str>, Vec<&str>) {
        (
            self.momentary.iter().map(String::as_str).collect(),
            self.toggled.iter().map(String::as_str).collect(),
        )
    }

    /// Profiles of the mappings that are currently held down.
    pub fn active_profiles(&self) -> Vec<&str> {
        self.active.iter().map(|a| a.profile.as_str()).collect()
    }

    pub fn set_front_app(&mut self, app: Option<String>) {
        self.front_app = app;
    }

    pub fn front_app(&self) -> Option<&str> {
        self.front_app.as_deref()
    }

    /// Swap in a new configuration without tearing down the session.
    ///
    /// Currently active mappings are released first so no key can get stuck.
    pub fn replace_config(&mut self, cfg: CompiledConfig) -> Vec<Output> {
        let mut outputs = Vec::new();
        for a in std::mem::take(&mut self.active) {
            outputs.extend(self.deactivate(a));
        }
        self.momentary.clear();
        self.toggled.clear();
        self.layer_sources.clear();
        self.pending = None;
        // A held-back Win press survives a reload. Forgetting it here would make
        // the next key lose the Win modifier (or, worse, deliver Win at the
        // wrong moment and pop the Start menu).
        if let Some(deferred) = self.deferred_meta {
            if !self.pressed.contains(&deferred.key) {
                self.deferred_meta = None;
            }
        }
        self.defer_meta = cfg.uses_meta_trigger();
        self.cfg = cfg;
        outputs
    }

    /// Feed a physical key transition, stamped with `now`.
    pub fn on_key_at(&mut self, key: KeyCode, state: KeyState, now: Instant) -> Outcome {
        self.stats.key_events += 1;
        match state {
            KeyState::Down => self.on_down(key, now),
            KeyState::Up => self.on_up(key),
        }
    }

    pub fn on_key(&mut self, key: KeyCode, state: KeyState) -> Outcome {
        self.on_key_at(key, state, Instant::now())
    }

    /// Timer work: resolve `tap_hold` deadlines, expire nothing else.
    pub fn on_tick(&mut self, now: Instant) -> Vec<Output> {
        if let Some(p) = &self.pending {
            if now >= p.deadline {
                return self.resolve_pending_hold();
            }
        }
        Vec::new()
    }

    /// How long the backend may sleep before calling [`Engine::on_tick`].
    pub fn time_until_deadline(&self, now: Instant) -> Option<Duration> {
        self.pending.as_ref().map(|p| p.deadline.saturating_duration_since(now))
    }

    // -- internals ----------------------------------------------------------

    fn on_down(&mut self, key: KeyCode, now: Instant) -> Outcome {
        if self.pressed.contains(&key) {
            return self.on_repeat(key);
        }

        let mut outputs = Vec::new();
        // Any other key pressed while a dual-role key is undecided means "hold".
        if let Some(p) = &self.pending {
            if p.key != key {
                outputs.extend(self.resolve_pending_hold());
            }
        }

        // Win is held back: Windows opens the Start menu on a lone Win tap and
        // the Feedback Hub on Win+F, so the shell must not see this key unless
        // the combination turns out not to be mapped.
        if self.defer_meta && is_meta(key) {
            self.pressed.push(key);
            self.deferred_meta = Some(DeferredMeta {
                key,
                consumed: false,
            });
            if let Some((pi, mi)) = self.resolve() {
                let profile = self.cfg.profiles[pi].name.clone();
                let map = self.cfg.profiles[pi].maps[mi].clone();
                if let Some(deferred) = &mut self.deferred_meta {
                    deferred.consumed = true;
                }
                outputs.extend(self.activate(&map, &profile, key, now));
                return Outcome {
                    swallow: true,
                    outputs,
                };
            }
            return Outcome {
                swallow: true,
                outputs,
            };
        }

        self.pressed.push(key);
        let resolved = self.resolve();

        // A held-back Win key: deliver it now unless this combination is the
        // mapping that needs it.
        if let Some(deferred) = self.deferred_meta {
            let used_by_mapping = resolved
                .map(|(pi, mi)| self.cfg.profiles[pi].maps[mi].uses_meta())
                .unwrap_or(false);
            if used_by_mapping {
                if let Some(d) = &mut self.deferred_meta {
                    d.consumed = true;
                }
            } else {
                self.deferred_meta = None;
                outputs.extend(self.deliver_meta(deferred.key));
            }
        }

        match resolved {
            Some((pi, mi)) => {
                let profile = self.cfg.profiles[pi].name.clone();
                let map = self.cfg.profiles[pi].maps[mi].clone();
                outputs.extend(self.activate(&map, &profile, key, now));
                Outcome {
                    swallow: true,
                    outputs,
                }
            }
            None => {
                if self.reinject_passthrough {
                    // The backend grabbed the device and dropped the original
                    // event, so we have to produce it ourselves.
                    self.emitted.insert(key, DownSource::Injected);
                    outputs.push(Output::KeyDown(key));
                    Outcome {
                        swallow: true,
                        outputs,
                    }
                } else {
                    // The hook lets the real event through: remember that the
                    // OS owns this key, but never inject a copy (that would
                    // type every character twice).
                    self.emitted.insert(key, DownSource::Physical);
                    Outcome {
                        swallow: false,
                        outputs,
                    }
                }
            }
        }
    }

    /// Hand a held-back Win press to the OS (native `Win+<key>` shortcuts).
    fn deliver_meta(&mut self, key: KeyCode) -> Vec<Output> {
        let mut outputs = Vec::new();
        self.press_once(key, &mut outputs);
        outputs
    }

    fn on_up(&mut self, key: KeyCode) -> Outcome {
        let mut outputs = Vec::new();
        let mut swallow = false;
        self.pressed.retain(|k| *k != key);

        // A Win press that never reached the shell: a lone tap has to be
        // replayed so the Start menu still works, unless a mapping used it.
        if let Some(deferred) = self.deferred_meta {
            if deferred.key == key {
                self.deferred_meta = None;
                if !deferred.consumed {
                    outputs.push(Output::KeyDown(key));
                    outputs.push(Output::KeyUp(key));
                }
                return Outcome {
                    swallow: true,
                    outputs,
                };
            }
        }

        // A dual-role key released before the timeout is a "tap".
        let tap_pending = matches!(&self.pending, Some(p) if p.key == key);
        if tap_pending {
            let p = self.pending.take().expect("checked above");
            outputs.extend(self.emit_tap(&p.tap));
            swallow = true;
        }

        // Release every active mapping whose trigger contains this key.
        let mut idx = self.active.len();
        while idx > 0 {
            idx -= 1;
            let hit = self.active[idx]
                .trigger
                .iter()
                .any(|m| m.matches(key));
            if hit {
                let a = self.active.remove(idx);
                outputs.extend(self.deactivate(a));
                swallow = true;
            }
        }

        match self.emitted.remove(&key) {
            // We produced the press (remap output, restored modifier, or a
            // re-injected key on a grabbed device): we owe the release, and the
            // physical release must not reach the OS as well.
            Some(DownSource::Injected) => {
                outputs.push(Output::KeyUp(key));
                swallow = true;
            }
            // The OS delivered the press itself, so it delivers the release too.
            Some(DownSource::Physical) => {}
            // The press was swallowed (mapping trigger, block, dual-role).
            None => swallow = true,
        }

        Outcome { swallow, outputs }
    }

    fn on_repeat(&mut self, key: KeyCode) -> Outcome {
        let mut outputs = Vec::new();
        let mut swallow = false;
        for a in &self.active {
            if a.trigger.iter().any(|m| m.matches(key)) {
                for k in &a.outputs {
                    outputs.push(Output::KeyDown(*k));
                }
                swallow = true;
            }
        }
        Outcome { swallow, outputs }
    }

    fn activate(
        &mut self,
        map: &CompiledMap,
        profile: &str,
        key: KeyCode,
        now: Instant,
    ) -> Vec<Output> {
        self.stats.activations += 1;
        self.last_activation = Some(format!(
            "{}: {} -> {}",
            map.origin,
            map.trigger_label(),
            map.action.describe()
        ));
        let mut outputs = Vec::new();

        match &map.action {
            Action::Remap(keys) => {
                // Modifiers that already reached the OS must be released so the
                // output is not polluted (classic "modifier clearing").
                let restore = self.clear_for_activation(&map.trigger, &mut outputs);
                let mut downs = Vec::new();
                for k in keys {
                    if self.press_once(*k, &mut outputs) {
                        downs.push(*k);
                    }
                }
                self.active.push(Active {
                    profile: profile.to_string(),
                    trigger: map.trigger.clone(),
                    outputs: downs,
                    restore,
                    layer: None,
                });
            }
            Action::Tap(keys) => {
                let restore = self.clear_for_activation(&map.trigger, &mut outputs);
                outputs.extend(self.emit_tap(keys));
                outputs.extend(self.restore_keys(&restore));
            }
            Action::Sequence(steps) => {
                // Each step is pressed and released before the next one starts,
                // e.g. ["shift+home", "backspace"].
                let restore = self.clear_for_activation(&map.trigger, &mut outputs);
                for step in steps {
                    let mut held = Vec::new();
                    for k in step {
                        if self.press_once(*k, &mut outputs) {
                            held.push(*k);
                        }
                    }
                    for k in held {
                        self.release_owned(k, &mut outputs);
                    }
                }
                outputs.extend(self.restore_keys(&restore));
            }
            // Actions that emit no keys never clear modifiers: the physical
            // modifiers keep working exactly as before.
            Action::Command(cmd) => {
                self.stats.commands += 1;
                outputs.push(Output::Command(cmd.clone()));
            }
            Action::Layer(name) => {
                self.momentary.push(name.clone());
                self.layer_sources.push(key);
                self.active.push(Active {
                    profile: profile.to_string(),
                    trigger: map.trigger.clone(),
                    outputs: Vec::new(),
                    restore: Vec::new(),
                    layer: Some((name.clone(), key)),
                });
            }
            Action::ToggleLayer(name) => match self.toggled.iter().position(|l| l == name) {
                Some(pos) => {
                    self.toggled.remove(pos);
                }
                None => self.toggled.push(name.clone()),
            },
            Action::Block => {}
            Action::TapHold {
                tap,
                hold,
                timeout_ms,
            } => {
                self.pending = Some(Pending {
                    key,
                    profile: profile.to_string(),
                    trigger: map.trigger.clone(),
                    tap: tap.clone(),
                    hold: hold.clone(),
                    deadline: now + Duration::from_millis(*timeout_ms),
                });
            }
        }
        outputs
    }

    /// Release everything that would otherwise leak into the output of the
    /// mapping that is about to activate:
    ///
    /// 1. the outputs of an *active* mapping whose trigger key takes part in
    ///    this combination — the `capslock -> leftctrl` case: `capslock + h`
    ///    must produce a clean Backspace, not `Ctrl+Backspace`;
    /// 2. trigger keys that already reached the OS (holding `Ctrl` before
    ///    `Ctrl+H` is completed).
    ///
    /// Everything released here is re-pressed when the mapping ends.
    ///
    /// Meta normally never gets here: a Win press that might be part of a
    /// mapping is held back by [`Engine::deferred_meta`], so the shell never
    /// sees it in the first place.
    fn clear_for_activation(
        &mut self,
        trigger: &[KeyMatcher],
        outputs: &mut Vec<Output>,
    ) -> Vec<KeyCode> {
        let mut restore = Vec::new();

        let involved: Vec<KeyCode> = self
            .pressed
            .iter()
            .copied()
            .filter(|k| trigger.iter().any(|m| m.matches(*k)))
            .collect();
        let owned: Vec<KeyCode> = self
            .active
            .iter()
            .filter(|a| {
                a.trigger
                    .iter()
                    .any(|m| involved.iter().any(|k| m.matches(*k)))
            })
            .flat_map(|a| a.outputs.iter().copied())
            .collect();
        for k in owned {
            if self.emitted.remove(&k).is_some() {
                restore.push(k);
                outputs.push(Output::KeyUp(k));
            }
        }

        // Physical trigger keys that the OS already knows about.
        for k in self.pressed.clone() {
            if !trigger.iter().any(|m| m.matches(k)) {
                continue;
            }
            if self.emitted.remove(&k).is_some() {
                restore.push(k);
                outputs.push(Output::KeyUp(k));
            }
        }

        restore
    }

    fn deactivate(&mut self, a: Active) -> Vec<Output> {
        let mut outputs = Vec::new();
        for k in a.outputs {
            self.release_owned(k, &mut outputs);
        }
        if let Some((layer, key)) = &a.layer {
            if let Some(pos) = self.momentary.iter().rposition(|l| l == layer) {
                self.momentary.remove(pos);
            }
            if let Some(pos) = self.layer_sources.iter().rposition(|k| k == key) {
                self.layer_sources.remove(pos);
            }
        }
        outputs.extend(self.restore_keys(&a.restore));
        outputs
    }

    /// Resolve an undecided dual-role key as "hold".
    fn resolve_pending_hold(&mut self) -> Vec<Output> {
        let p = match self.pending.take() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let mut outputs = Vec::new();
        let mut downs = Vec::new();
        for k in &p.hold {
            if self.press_once(*k, &mut outputs) {
                downs.push(*k);
            }
        }
        self.active.push(Active {
            profile: p.profile,
            trigger: p.trigger,
            outputs: downs,
            restore: Vec::new(),
            layer: None,
        });
        outputs
    }

    /// Press and immediately release a chord.
    fn emit_tap(&mut self, keys: &[KeyCode]) -> Vec<Output> {
        let mut outputs = Vec::new();
        let mut pressed = Vec::new();
        for k in keys {
            if self.press_once(*k, &mut outputs) {
                pressed.push(*k);
            }
        }
        for k in pressed {
            self.release_owned(k, &mut outputs);
        }
        outputs
    }

    /// Should a key that was cleared for a combination go back down when the
    /// combination ends? Either the user still holds it physically, or another
    /// active mapping owns it — the `capslock -> leftctrl` mapping keeps
    /// "holding" the ctrl that was released for `capslock + h`.
    fn is_wanted(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key) || self.active.iter().any(|a| a.outputs.contains(&key))
    }

    /// Re-press modifiers that were cleared for a combination and are still
    /// needed.
    fn restore_keys(&mut self, keys: &[KeyCode]) -> Vec<Output> {
        let mut outputs = Vec::new();
        for k in keys {
            if self.is_wanted(*k) {
                self.press_once(*k, &mut outputs);
            }
        }
        outputs
    }

    /// Layer order: momentary (top first), toggled (most recent first), base.
    fn resolve(&self) -> Option<(usize, usize)> {
        let mut order: Vec<&str> = Vec::new();
        for name in self.momentary.iter().rev() {
            let name = name.as_str();
            if !order.contains(&name) {
                order.push(name);
            }
        }
        for name in self.toggled.iter().rev() {
            let name = name.as_str();
            if !order.contains(&name) {
                order.push(name);
            }
        }
        let base = self.cfg.general.default_profile.as_str();
        if !order.contains(&base) {
            order.push(base);
        }

        for name in order {
            let pi = match self.cfg.profiles.iter().position(|p| p.name == name) {
                Some(i) => i,
                None => continue,
            };
            let profile = &self.cfg.profiles[pi];
            for (mi, map) in profile.maps.iter().enumerate() {
                if !map.app_allows(self.front_app.as_deref()) {
                    continue;
                }
                if self.trigger_matches(&map.trigger) {
                    return Some((pi, mi));
                }
            }
        }
        None
    }

    /// Keys considered for combo matching: everything physically held except
    /// the keys that only exist to hold a momentary layer open.
    fn matching_keys(&self) -> Vec<KeyCode> {
        self.pressed
            .iter()
            .copied()
            .filter(|k| !self.layer_sources.contains(k))
            .collect()
    }

    fn trigger_matches(&self, trigger: &[KeyMatcher]) -> bool {
        let pressed = self.matching_keys();
        if trigger.len() > pressed.len() {
            return false;
        }
        if self.cfg.general.combo_mode == ComboMode::Exact && trigger.len() != pressed.len() {
            return false;
        }
        let mut used = vec![false; pressed.len()];
        assign(trigger, 0, &pressed, &mut used)
    }
}

/// A Meta (Win) press that was swallowed before reaching the shell.
///
/// Windows reacts to the Win key itself: a lone tap opens the Start menu and
/// `Win+F` opens the Feedback Hub, so a `meta + X` mapping must keep the shell
/// from ever seeing Win. The press is therefore held here and only delivered if
/// it turns out not to be part of a mapping (Win+E, Win+L, ... still work).
#[derive(Debug, Clone, Copy)]
struct DeferredMeta {
    key: KeyCode,
    /// A mapping that uses Meta has fired, so Win must stay invisible — not
    /// even a synthetic tap on release.
    consumed: bool,
}

fn is_meta(key: KeyCode) -> bool {
    matches!(key, KeyCode::LeftMeta | KeyCode::RightMeta)
}

/// Backtracking search: can every matcher be satisfied by a distinct key?
fn assign(trigger: &[KeyMatcher], i: usize, pressed: &[KeyCode], used: &mut Vec<bool>) -> bool {
    if i == trigger.len() {
        return true;
    }
    for (j, k) in pressed.iter().enumerate() {
        if used[j] || !trigger[i].matches(*k) {
            continue;
        }
        used[j] = true;
        if assign(trigger, i + 1, pressed, used) {
            return true;
        }
        used[j] = false;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CompiledConfig;

    fn engine(toml_src: &str) -> Engine {
        let cfg = CompiledConfig::from_toml_str(toml_src).expect("config");
        Engine::new(cfg)
    }

    fn keys(out: &Outcome) -> Vec<String> {
        outs(&out.outputs)
    }

    fn outs(list: &[Output]) -> Vec<String> {
        list.iter()
            .map(|o| match o {
                Output::KeyDown(k) => format!("+{k}"),
                Output::KeyUp(k) => format!("-{k}"),
                Output::Command(c) => format!("!{c}"),
            })
            .collect()
    }

    const SIMPLE: &str = r#"
[general]
default_profile = "default"

[profiles.default]
[[profiles.default.maps]]
keys = ["rightalt"]
remap = "rightctrl"

[[profiles.default.maps]]
keys = ["ctrl", "h"]
remap = "left"

[[profiles.default.maps]]
keys = ["ctrl", "alt", "delete"]
command = "boom"

[[profiles.default.maps]]
key = "insert"
block = true
"#;

    #[test]
    fn plain_key_passes_through_without_duplicating() {
        let mut e = engine(SIMPLE);
        let d = e.on_key(KeyCode::A, KeyState::Down);
        assert!(!d.swallow);
        assert!(
            d.outputs.is_empty(),
            "the OS delivers the physical key itself; injecting it again would type `aa`"
        );
        let u = e.on_key(KeyCode::A, KeyState::Up);
        assert!(!u.swallow);
        assert!(u.outputs.is_empty());
    }

    /// Linux (evdev grab) drops the original event, so there the engine has to
    /// produce the pass-through itself — and it must swallow the original.
    #[test]
    fn grabbed_backends_reinject_passthrough_keys() {
        let mut e = engine(SIMPLE);
        e.set_reinject_passthrough(true);
        let d = e.on_key(KeyCode::A, KeyState::Down);
        assert!(d.swallow, "the original must not be delivered twice");
        assert_eq!(keys(&d), vec!["+a"]);
        let u = e.on_key(KeyCode::A, KeyState::Up);
        assert!(u.swallow);
        assert_eq!(keys(&u), vec!["-a"]);
    }

    #[test]
    fn single_key_remap_holds() {
        let mut e = engine(SIMPLE);
        let d = e.on_key(KeyCode::RightAlt, KeyState::Down);
        assert!(d.swallow);
        assert_eq!(keys(&d), vec!["+rightctrl"]);
        let u = e.on_key(KeyCode::RightAlt, KeyState::Up);
        assert!(u.swallow);
        assert_eq!(keys(&u), vec!["-rightctrl"]);
    }

    #[test]
    fn combo_clears_modifier_already_sent() {
        let mut e = engine(SIMPLE);
        let c = e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        assert!(!c.swallow, "ctrl alone is not part of any complete combo");
        assert!(
            c.outputs.is_empty(),
            "an unmodified modifier is delivered by the OS itself"
        );

        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert!(h.swallow);
        assert_eq!(
            keys(&h),
            vec!["-leftctrl", "+left"],
            "the already delivered ctrl must be released first"
        );

        let h_up = e.on_key(KeyCode::H, KeyState::Up);
        assert_eq!(
            keys(&h_up),
            vec!["-left", "+leftctrl"],
            "the trigger key release also restores what was cleared"
        );

        let c_up = e.on_key(KeyCode::LeftCtrl, KeyState::Up);
        assert!(
            c_up.swallow,
            "the restored ctrl is ours, so we release it instead of the physical event"
        );
        assert_eq!(keys(&c_up), vec!["-leftctrl"]);
    }

    #[test]
    fn right_ctrl_variant_of_generic_combo() {
        let mut e = engine(SIMPLE);
        let c = e.on_key(KeyCode::RightCtrl, KeyState::Down);
        assert!(c.outputs.is_empty());
        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert_eq!(keys(&h), vec!["-rightctrl", "+left"]);
    }

    #[test]
    fn command_fires_once_and_swallows() {
        let mut e = engine(SIMPLE);
        e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        e.on_key(KeyCode::LeftAlt, KeyState::Down);
        let d = e.on_key(KeyCode::Delete, KeyState::Down);
        assert!(d.swallow);
        assert_eq!(
            keys(&d),
            vec!["!boom"],
            "a command emits no keys and must not disturb held modifiers"
        );
        let u = e.on_key(KeyCode::Delete, KeyState::Up);
        assert!(u.swallow);
        assert!(u.outputs.is_empty());
    }

    #[test]
    fn block_swallows_both_edges() {
        let mut e = engine(SIMPLE);
        let d = e.on_key(KeyCode::Insert, KeyState::Down);
        assert!(d.swallow);
        assert!(d.outputs.is_empty());
        let u = e.on_key(KeyCode::Insert, KeyState::Up);
        assert!(u.swallow);
        assert!(u.outputs.is_empty());
    }

    const DUAL: &str = r#"
[general]
default_profile = "default"
tap_hold_ms = 200

[profiles.default]
[[profiles.default.maps]]
key = "capslock"
tap_hold = { tap = "escape", hold = "leftctrl" }

[[profiles.default.maps]]
key = "f1"
layer = "nav"

[profiles.nav]
[[profiles.nav.maps]]
key = "h"
remap = "left"
"#;

    #[test]
    fn tap_hold_tap_when_released_early() {
        let mut e = engine(DUAL);
        let t0 = Instant::now();
        let d = e.on_key_at(KeyCode::CapsLock, KeyState::Down, t0);
        assert!(d.swallow && d.outputs.is_empty());
        let u = e.on_key_at(KeyCode::CapsLock, KeyState::Up, t0 + Duration::from_millis(50));
        assert!(u.swallow);
        assert_eq!(keys(&u), vec!["+escape", "-escape"]);
    }

    #[test]
    fn tap_hold_holds_after_timeout() {
        let mut e = engine(DUAL);
        let t0 = Instant::now();
        e.on_key_at(KeyCode::CapsLock, KeyState::Down, t0);
        let tick = e.on_tick(t0 + Duration::from_millis(250));
        assert_eq!(outs(&tick), vec!["+leftctrl"]);
        let u = e.on_key_at(KeyCode::CapsLock, KeyState::Up, t0 + Duration::from_millis(300));
        assert_eq!(keys(&u), vec!["-leftctrl"]);
    }

    #[test]
    fn tap_hold_holds_when_another_key_arrives() {
        let mut e = engine(DUAL);
        let t0 = Instant::now();
        e.on_key_at(KeyCode::CapsLock, KeyState::Down, t0);
        let a = e.on_key_at(KeyCode::A, KeyState::Down, t0 + Duration::from_millis(30));
        assert!(!a.swallow, "plain keys still reach the OS");
        assert_eq!(
            keys(&a),
            vec!["+leftctrl"],
            "the hold modifier is injected, the physical `a` is left to the OS"
        );
    }

    #[test]
    fn momentary_layer_shadows_and_falls_through() {
        let mut e = engine(DUAL);
        let t0 = Instant::now();
        let d = e.on_key_at(KeyCode::F1, KeyState::Down, t0);
        assert!(d.swallow);
        let (momentary, _) = e.layers();
        assert_eq!(momentary, vec!["nav"]);

        let h = e.on_key_at(KeyCode::H, KeyState::Down, t0);
        assert_eq!(keys(&h), vec!["+left"]);
        let h_up = e.on_key_at(KeyCode::H, KeyState::Up, t0);
        assert_eq!(keys(&h_up), vec!["-left"]);

        // Not defined in nav: falls through to the base profile.
        let a = e.on_key_at(KeyCode::A, KeyState::Down, t0);
        assert!(!a.swallow);
        assert!(a.outputs.is_empty());

        let up = e.on_key_at(KeyCode::F1, KeyState::Up, t0);
        assert!(up.outputs.is_empty());
        let (momentary, _) = e.layers();
        assert!(momentary.is_empty());
    }

    #[test]
    fn toggle_layer_persists() {
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
key = "f2"
toggle_layer = "nav"
[profiles.nav]
[[profiles.nav.maps]]
key = "h"
remap = "left"
"#;
        let mut e = engine(src);
        e.on_key(KeyCode::F2, KeyState::Down);
        e.on_key(KeyCode::F2, KeyState::Up);
        let (_, toggled) = e.layers();
        assert_eq!(toggled, vec!["nav"]);
        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert_eq!(keys(&h), vec!["+left"]);
        e.on_key(KeyCode::H, KeyState::Up);
        e.on_key(KeyCode::F2, KeyState::Down);
        e.on_key(KeyCode::F2, KeyState::Up);
        let (_, toggled) = e.layers();
        assert!(toggled.is_empty());
        let h2 = e.on_key(KeyCode::H, KeyState::Down);
        assert!(!h2.swallow);
    }

    #[test]
    fn autorepeat_repeats_output() {
        let mut e = engine(SIMPLE);
        e.on_key(KeyCode::RightAlt, KeyState::Down);
        let r = e.on_key(KeyCode::RightAlt, KeyState::Down);
        assert!(r.swallow);
        assert_eq!(keys(&r), vec!["+rightctrl"]);
    }

    #[test]
    fn app_filter_disables_mapping() {
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
keys = ["ctrl", "h"]
remap = "left"
only_in = ["firefox"]
"#;
        let mut e = engine(src);
        e.set_front_app(Some("Code.exe".into()));
        let d = e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        assert!(!d.swallow);
        e.on_key(KeyCode::H, KeyState::Down);
        // no mapping matched -> both keys are delivered
        assert!(e.pressed().contains(&KeyCode::H));
        e.set_front_app(Some("firefox.exe".into()));
        e.on_key(KeyCode::H, KeyState::Up);
        e.on_key(KeyCode::LeftCtrl, KeyState::Up);
        e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert!(h.swallow);
    }

    #[test]
    fn subset_mode_allows_extra_keys() {
        let src = r#"
[general]
default_profile = "default"
combo_mode = "subset"
[profiles.default]
[[profiles.default.maps]]
keys = ["ctrl", "h"]
remap = "left"
"#;
        let mut e = engine(src);
        e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        e.on_key(KeyCode::A, KeyState::Down);
        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert!(h.swallow);
    }

    #[test]
    fn config_swap_releases_active_keys() {
        let mut e = engine(SIMPLE);
        e.on_key(KeyCode::RightAlt, KeyState::Down);
        let cfg = CompiledConfig::from_toml_str(SIMPLE).unwrap();
        let outs = e.replace_config(cfg);
        assert_eq!(
            outs,
            vec![Output::KeyUp(KeyCode::RightCtrl)],
            "no key may stay stuck after a reload"
        );
    }

    #[test]
    fn chord_remap_presses_and_releases_all() {
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
key = "f9"
remap = "ctrl+shift+t"
[[profiles.default.maps]]
key = "f10"
tap = "ctrl+shift+p"
"#;
        let mut e = engine(src);
        let d = e.on_key(KeyCode::F9, KeyState::Down);
        assert_eq!(keys(&d), vec!["+leftctrl", "+leftshift", "+t"]);
        let u = e.on_key(KeyCode::F9, KeyState::Up);
        assert_eq!(keys(&u), vec!["-leftctrl", "-leftshift", "-t"]);
        let t = e.on_key(KeyCode::F10, KeyState::Down);
        assert_eq!(
            keys(&t),
            vec!["+leftctrl", "+leftshift", "+p", "-leftctrl", "-leftshift", "-p"]
        );
        let t_up = e.on_key(KeyCode::F10, KeyState::Up);
        assert!(t_up.swallow && t_up.outputs.is_empty());
    }

    // -- caps lock / control swap (the "emacs hands" layout) -----------------

    const SWAP: &str = r#"
[general]
default_profile = "default"

[profiles.default]
[[profiles.default.maps]]
keys = ["capslock", "h"]
remap = "backspace"

[[profiles.default.maps]]
keys = ["meta", "f"]
remap = "leftctrl+f"

[[profiles.default.maps]]
key = "capslock"
remap = "leftctrl"

[[profiles.default.maps]]
key = "leftctrl"
remap = "capslock"
"#;

    #[test]
    fn capslock_acts_as_ctrl() {
        let mut e = engine(SWAP);
        let caps = e.on_key(KeyCode::CapsLock, KeyState::Down);
        assert!(caps.swallow);
        assert_eq!(keys(&caps), vec!["+leftctrl"]);

        // Any other key keeps its normal meaning with ctrl held (e.g. Ctrl+C).
        let c = e.on_key(KeyCode::C, KeyState::Down);
        assert!(!c.swallow, "an unrelated key is passed through by the OS");
        assert!(c.outputs.is_empty());
        e.on_key(KeyCode::C, KeyState::Up);

        let caps_up = e.on_key(KeyCode::CapsLock, KeyState::Up);
        assert!(caps_up.swallow);
        assert_eq!(keys(&caps_up), vec!["-leftctrl"]);
    }

    #[test]
    fn swap_combination_releases_the_injected_ctrl() {
        let mut e = engine(SWAP);
        e.on_key(KeyCode::CapsLock, KeyState::Down);

        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert!(h.swallow);
        assert_eq!(
            keys(&h),
            vec!["-leftctrl", "+backspace"],
            "capslock+h must be a clean Backspace, not Ctrl+Backspace"
        );

        let h_up = e.on_key(KeyCode::H, KeyState::Up);
        assert_eq!(
            keys(&h_up),
            vec!["-backspace", "+leftctrl"],
            "ctrl is restored because capslock is still held"
        );

        // ... and it is still a real ctrl afterwards.
        let c = e.on_key(KeyCode::C, KeyState::Down);
        assert!(c.outputs.is_empty());
        e.on_key(KeyCode::C, KeyState::Up);

        let caps_up = e.on_key(KeyCode::CapsLock, KeyState::Up);
        assert_eq!(keys(&caps_up), vec!["-leftctrl"]);
    }

    #[test]
    fn physical_ctrl_becomes_capslock() {
        let mut e = engine(SWAP);
        let d = e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        assert!(d.swallow);
        assert_eq!(keys(&d), vec!["+capslock"]);
        let u = e.on_key(KeyCode::LeftCtrl, KeyState::Up);
        assert!(u.swallow);
        assert_eq!(keys(&u), vec!["-capslock"]);
    }

    #[test]
    fn win_f_emits_a_real_ctrl_f() {
        let mut e = engine(SWAP);

        // Win is held back so the shell never sees it: no Start menu, no
        // Feedback Hub.
        let meta = e.on_key(KeyCode::LeftMeta, KeyState::Down);
        assert!(meta.swallow);
        assert!(meta.outputs.is_empty(), "{:?}", keys(&meta));

        let f = e.on_key(KeyCode::F, KeyState::Down);
        assert!(f.swallow);
        assert_eq!(
            keys(&f),
            vec!["+leftctrl", "+f"],
            "a clean Ctrl+F: Win was never delivered"
        );

        let f_up = e.on_key(KeyCode::F, KeyState::Up);
        assert_eq!(keys(&f_up), vec!["-leftctrl", "-f"]);

        // The mapping consumed the Win key: releasing it must stay silent.
        let meta_up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        assert!(meta_up.swallow);
        assert!(meta_up.outputs.is_empty(), "{:?}", keys(&meta_up));
    }

    #[test]
    fn unmapped_win_combinations_still_reach_windows() {
        let mut e = engine(SWAP);
        // Win alone keeps working as the Start menu key.
        let down = e.on_key(KeyCode::LeftMeta, KeyState::Down);
        assert!(down.swallow);
        let up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        assert!(up.swallow);
        assert_eq!(keys(&up), vec!["+leftmeta", "-leftmeta"]);

        // Win + E (not mapped) is delivered late but must still work.
        e.on_key(KeyCode::LeftMeta, KeyState::Down);
        let e_key = e.on_key(KeyCode::E, KeyState::Down);
        assert!(!e_key.swallow, "the OS delivers the physical E");
        assert_eq!(keys(&e_key), vec!["+leftmeta"], "Win is delivered first");
        let e_up = e.on_key(KeyCode::E, KeyState::Up);
        assert!(!e_up.swallow && e_up.outputs.is_empty());
        let meta_up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        assert!(meta_up.swallow);
        assert_eq!(keys(&meta_up), vec!["-leftmeta"]);
    }

    #[test]
    fn win_alone_can_be_disabled_explicitly() {
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
keys = ["meta"]
block = true
"#;
        let mut e = engine(src);
        let down = e.on_key(KeyCode::LeftMeta, KeyState::Down);
        assert!(down.swallow);
        let up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        assert!(up.swallow);
        assert!(
            up.outputs.is_empty(),
            "blocking Win must suppress the Start menu tap"
        );
    }

    #[test]
    fn shutdown_releases_held_keys() {
        // CapsLock held: the injected left ctrl must not stay down.
        let mut e = engine(SWAP);
        e.on_key(KeyCode::CapsLock, KeyState::Down);
        let released = e.release_all();
        assert!(
            outs(&released).contains(&"-leftctrl".to_string()),
            "{:?}",
            outs(&released)
        );
        assert!(e.release_all().is_empty(), "release_all is idempotent");

        // CapsLock+H held: the combination's own output is down as well.
        let mut e = engine(SWAP);
        e.on_key(KeyCode::CapsLock, KeyState::Down);
        e.on_key(KeyCode::H, KeyState::Down);
        let labels = outs(&e.release_all());
        assert!(labels.contains(&"-backspace".to_string()), "{labels:?}");
        assert!(e.release_all().is_empty());

        let (momentary, toggled) = e.layers();
        assert!(momentary.is_empty() && toggled.is_empty());
    }

    /// The layout the user actually runs: `["ctrl", X]` triggers (which match
    /// the *physical* Ctrl keys) plus `capslock -> leftctrl` and no reverse
    /// mapping. Documents both halves of the behaviour.
    const GENERIC_CTRL: &str = r#"
[general]
default_profile = "default"

[profiles.default]
[[profiles.default.maps]]
keys = ["ctrl", "h"]
remap = "backspace"

[[profiles.default.maps]]
key = "capslock"
remap = "leftctrl"
"#;

    #[test]
    fn generic_ctrl_combos_fire_from_the_physical_ctrl_keys() {
        let mut e = engine(GENERIC_CTRL);

        // CapsLock alone is a clean Ctrl: it must not toggle capitalisation.
        let caps = e.on_key(KeyCode::CapsLock, KeyState::Down);
        assert!(caps.swallow, "the physical CapsLock key must be swallowed");
        assert_eq!(keys(&caps), vec!["+leftctrl"]);
        let caps_up = e.on_key(KeyCode::CapsLock, KeyState::Up);
        assert_eq!(keys(&caps_up), vec!["-leftctrl"]);

        // The map matches the physical control key, not the remapped one.
        let c = e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        assert!(!c.swallow && c.outputs.is_empty());
        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert!(h.swallow);
        assert_eq!(keys(&h), vec!["-leftctrl", "+backspace"]);
        let h_up = e.on_key(KeyCode::H, KeyState::Up);
        assert_eq!(keys(&h_up), vec!["-backspace", "+leftctrl"]);
        e.on_key(KeyCode::LeftCtrl, KeyState::Up);
    }

    #[test]
    fn generic_ctrl_combos_do_not_fire_from_capslock() {
        let mut e = engine(GENERIC_CTRL);
        let caps = e.on_key(KeyCode::CapsLock, KeyState::Down);
        assert_eq!(keys(&caps), vec!["+leftctrl"]);

        // CapsLock + H: no mapping matches, so the app receives a real Ctrl+H
        // (which a terminal turns into Backspace on its own — that is why this
        // layout looked like it worked, but only inside a terminal).
        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert!(!h.swallow, "unmapped: the OS delivers the physical H");
        assert!(h.outputs.is_empty());
        assert_eq!(e.active_profiles().len(), 1, "only the capslock mapping is active");
    }

    #[test]
    fn meta_actions_without_output_keys_still_cancel_the_start_menu() {
        // A `win + q` that only runs a command must not open the Start menu:
        // Win never reaches the shell in the first place.
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
keys = ["meta", "q"]
command = "echo hi"
[[profiles.default.maps]]
keys = ["meta", "w"]
block = true
"#;
        let mut e = engine(src);
        let meta = e.on_key(KeyCode::LeftMeta, KeyState::Down);
        assert!(meta.swallow && meta.outputs.is_empty());

        let q = e.on_key(KeyCode::Q, KeyState::Down);
        assert!(q.swallow);
        assert_eq!(keys(&q), vec!["!echo hi"], "no key juggling needed");

        let q_up = e.on_key(KeyCode::Q, KeyState::Up);
        assert!(q_up.swallow && q_up.outputs.is_empty());

        let w = e.on_key(KeyCode::W, KeyState::Down);
        assert!(w.swallow && w.outputs.is_empty());

        let meta_up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        assert!(meta_up.swallow && meta_up.outputs.is_empty());
    }

    #[test]
    fn plain_actions_do_not_disturb_other_modifiers() {
        let mut e = engine(SIMPLE);
        e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        e.on_key(KeyCode::LeftAlt, KeyState::Down);
        let d = e.on_key(KeyCode::Delete, KeyState::Down);
        // No meta in the trigger -> no neutral tap, no spurious releases.
        assert_eq!(keys(&d), vec!["!boom"]);
    }

    #[test]
    fn dual_role_capslock_keeps_capitalisation_and_ctrl() {
        // The layout suggested when the user wants both: tap = CapsLock,
        // hold = Ctrl, plus combinations that trigger on the physical key.
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
keys = ["capslock", "h"]
remap = "backspace"
[[profiles.default.maps]]
key = "capslock"
tap_hold = { tap = "capslock", hold = "leftctrl", timeout_ms = 180 }
"#;
        let mut e = engine(src);

        // Quick tap: capitalisation still works.
        let down = e.on_key(KeyCode::CapsLock, KeyState::Down);
        assert!(down.swallow && down.outputs.is_empty());
        let up = e.on_key(KeyCode::CapsLock, KeyState::Up);
        assert_eq!(keys(&up), vec!["+capslock", "-capslock"]);

        // Hold + h: a clean Backspace, so capitalisation survives the hold.
        let down = e.on_key(KeyCode::CapsLock, KeyState::Down);
        assert!(down.swallow && down.outputs.is_empty());
        let h = e.on_key(KeyCode::H, KeyState::Down);
        assert!(h.swallow);
        assert_eq!(keys(&h), vec!["+leftctrl", "-leftctrl", "+backspace"]);
        let h_up = e.on_key(KeyCode::H, KeyState::Up);
        assert_eq!(keys(&h_up), vec!["-backspace", "+leftctrl"]);
        let caps_up = e.on_key(KeyCode::CapsLock, KeyState::Up);
        assert_eq!(keys(&caps_up), vec!["-leftctrl"]);
    }

    #[test]
    fn reload_while_win_is_held_keeps_it_invisible() {
        let mut e = engine(SWAP);
        let down = e.on_key(KeyCode::LeftMeta, KeyState::Down);
        assert!(down.swallow && down.outputs.is_empty());

        // The user saves the config file while still holding Win.
        let cfg = CompiledConfig::from_toml_str(SWAP).unwrap();
        let released = e.replace_config(cfg);
        assert!(released.is_empty(), "{:?}", outs(&released));

        let mut seen: Vec<String> = Vec::new();
        let f = e.on_key(KeyCode::F, KeyState::Down);
        seen.extend(keys(&f));
        assert!(f.swallow);
        assert_eq!(
            keys(&f),
            vec!["+leftctrl", "+f"],
            "a reload must not make Win leak into the output"
        );
        let f_up = e.on_key(KeyCode::F, KeyState::Up);
        seen.extend(keys(&f_up));
        let meta_up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        seen.extend(keys(&meta_up));
        assert!(meta_up.swallow);
        assert!(
            !seen.iter().any(|o| o.contains("leftmeta")),
            "Win must never be injected while it is part of a mapping: {seen:?}"
        );
    }

    #[test]
    fn reload_does_not_swallow_an_in_flight_win_press() {
        // Regression: reloading used to forget the held-back Win press, so the
        // next key silently lost the Win modifier instead of reaching Windows.
        let mut e = engine(SWAP);
        e.on_key(KeyCode::LeftMeta, KeyState::Down);

        // Now the config loses its meta mappings, then the user presses E.
        let plain = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
key = "f1"
remap = "f2"
"#;
        let cfg = CompiledConfig::from_toml_str(plain).unwrap();
        e.replace_config(cfg);

        let e_key = e.on_key(KeyCode::E, KeyState::Down);
        assert_eq!(
            keys(&e_key),
            vec!["+leftmeta"],
            "the held-back Win press has to be delivered once it is no longer mapped"
        );
        assert!(!e_key.swallow);
        let meta_up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        assert_eq!(keys(&meta_up), vec!["-leftmeta"]);
    }

    #[test]
    fn reload_while_a_win_mapping_is_active_releases_cleanly() {
        let mut e = engine(SWAP);
        e.on_key(KeyCode::LeftMeta, KeyState::Down);
        let f = e.on_key(KeyCode::F, KeyState::Down);
        assert_eq!(keys(&f), vec!["+leftctrl", "+f"]);

        let cfg = CompiledConfig::from_toml_str(SWAP).unwrap();
        let released = e.replace_config(cfg);
        assert_eq!(
            outs(&released),
            vec!["-leftctrl", "-f"],
            "the reload must give back what the mapping had pressed"
        );

        // The user is still holding Win+F: releasing Win ends the mapping and
        // must not inject Win itself.
        let meta_up = e.on_key(KeyCode::LeftMeta, KeyState::Up);
        assert!(meta_up.swallow);
        let labels = keys(&meta_up);
        assert!(
            !labels.iter().any(|o| o.contains("leftmeta")),
            "no Win event may be injected: {labels:?}"
        );
        let f_up = e.on_key(KeyCode::F, KeyState::Up);
        assert!(f_up.swallow && f_up.outputs.is_empty());
    }

    #[test]
    fn sequence_runs_every_step_in_order() {
        // "delete to the beginning of the line": select, then delete.
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
keys = ["ctrl", "u"]
sequence = ["shift+home", "backspace"]
"#;
        let mut e = engine(src);
        e.on_key(KeyCode::LeftCtrl, KeyState::Down);
        let u = e.on_key(KeyCode::U, KeyState::Down);
        assert!(u.swallow);
        assert_eq!(
            keys(&u),
            vec![
                "-leftctrl", // the ctrl that was already delivered is cleared
                "+leftshift",
                "+home",
                "-leftshift",
                "-home",
                "+backspace",
                "-backspace",
                "+leftctrl", // ... and restored at the end, the user still holds it
            ]
        );
        // Nothing stays pressed.
        let u_up = e.on_key(KeyCode::U, KeyState::Up);
        assert!(u_up.swallow && u_up.outputs.is_empty());
        let ctrl_up = e.on_key(KeyCode::LeftCtrl, KeyState::Up);
        assert!(ctrl_up.swallow, "the restored ctrl is ours to release");
        assert_eq!(keys(&ctrl_up), vec!["-leftctrl"]);
    }

    #[test]
    fn sequence_uses_physical_modifiers_as_is() {
        // The user already holds Shift: the step must not press or release it.
        // (subset mode, because an exactly matching combination would ignore a
        // single-key trigger while Shift is down.)
        let src = r#"
[general]
default_profile = "default"
combo_mode = "subset"
[profiles.default]
[[profiles.default.maps]]
key = "f8"
sequence = ["shift+home", "backspace"]
"#;
        let mut e = engine(src);
        e.on_key(KeyCode::LeftShift, KeyState::Down);
        let f8 = e.on_key(KeyCode::F8, KeyState::Down);
        assert!(f8.swallow);
        assert_eq!(
            keys(&f8),
            vec!["+home", "-home", "+backspace", "-backspace"],
            "the held shift must not be disturbed"
        );
    }

    #[test]
    fn single_step_sequence_behaves_like_a_tap() {
        let src = r#"
[general]
default_profile = "default"
[profiles.default]
[[profiles.default.maps]]
key = "f7"
sequence = "ctrl+shift+p"
"#;
        let mut e = engine(src);
        let d = e.on_key(KeyCode::F7, KeyState::Down);
        assert_eq!(
            keys(&d),
            vec!["+leftctrl", "+leftshift", "+p", "-leftctrl", "-leftshift", "-p"]
        );
    }

    #[test]
    fn shipped_swap_layout_is_valid() {        let cfg = CompiledConfig::from_toml_str(include_str!("../../../examples/caps-ctrl-emacs.toml"))
            .expect("the shipped caps/ctrl layout must compile");
        assert!(cfg.map_count() >= 10);
    }
}
