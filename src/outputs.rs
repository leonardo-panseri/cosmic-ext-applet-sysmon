// SPDX-License-Identifier: MPL-2.0

//! The outputs' logical sizes, from the Wayland output events iced forwards to subscriptions,
//! so the popup knows how much room its output leaves it ([`crate::ui::columns`]). The panel
//! names the applet's output in `COSMIC_PANEL_OUTPUT` (libcosmic's `applet::Context::output_name`).

/// The most outputs remembered (D8).
const MAX_OUTPUTS: usize = 16;

/// One output, as last announced.
#[derive(Debug, Clone, PartialEq)]
struct Output<K> {
    key: K,
    name: Option<String>,
    /// Width and height, logical pixels: scaled and rotated.
    size: Option<(f32, f32)>,
}

/// The outputs, by a key that tells them apart (the `wl_output` itself in the applet).
#[derive(Debug, Clone)]
pub struct Outputs<K> {
    outputs: Vec<Output<K>>,
}

impl<K> Default for Outputs<K> {
    fn default() -> Self {
        Self {
            outputs: Vec::new(),
        }
    }
}

impl<K: PartialEq> Outputs<K> {
    /// Records that the output `key` is called `name` and is `size` big, logical pixels.
    pub fn update(&mut self, key: K, name: Option<String>, size: Option<(i32, i32)>) {
        #[expect(clippy::cast_precision_loss, reason = "screen sizes")]
        let size = size
            .filter(|&(width, height)| width > 0 && height > 0)
            .map(|(width, height)| (width as f32, height as f32));
        let output = Output { key, name, size };
        if let Some(known) = self.outputs.iter_mut().find(|o| o.key == output.key) {
            *known = output;
        } else if self.outputs.len() < MAX_OUTPUTS {
            self.outputs.push(output);
        }
    }

    /// Forgets the output `key`.
    pub fn remove(&mut self, key: &K) {
        self.outputs.retain(|o| o.key != *key);
    }

    /// The logical size of the output called `name`. Without a name, as outside the panel, the
    /// only output's, if there is just one.
    #[must_use]
    pub fn size(&self, name: &str) -> Option<(f32, f32)> {
        let output = if name.is_empty() {
            match self.outputs.as_slice() {
                [only] => Some(only),
                _ => None,
            }
        } else {
            self.outputs
                .iter()
                .find(|o| o.name.as_deref() == Some(name))
        };
        output.and_then(|o| o.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_s_output_is_found_by_name() {
        let mut outputs = Outputs::default();
        outputs.update(1, Some("eDP-1".into()), Some((1920, 1200)));
        outputs.update(2, Some("DP-2".into()), Some((2560, 1440)));
        assert_eq!(outputs.size("DP-2"), Some((2560.0, 1440.0)));
        assert_eq!(outputs.size("eDP-1"), Some((1920.0, 1200.0)));
        assert_eq!(outputs.size("HDMI-A-1"), None);
    }

    #[test]
    fn without_a_name_only_a_single_output_counts() {
        let mut outputs = Outputs::default();
        outputs.update(1, Some("eDP-1".into()), Some((1920, 1200)));
        assert_eq!(outputs.size(""), Some((1920.0, 1200.0)));
        outputs.update(2, Some("DP-2".into()), Some((2560, 1440)));
        assert_eq!(outputs.size(""), None);
    }

    #[test]
    fn a_changed_output_replaces_its_size_and_a_removed_one_is_forgotten() {
        let mut outputs = Outputs::default();
        outputs.update(1, Some("eDP-1".into()), Some((1920, 1200)));
        // Scaled to 150 %.
        outputs.update(1, Some("eDP-1".into()), Some((1280, 800)));
        assert_eq!(outputs.size("eDP-1"), Some((1280.0, 800.0)));
        outputs.remove(&1);
        assert_eq!(outputs.size("eDP-1"), None);
    }
}
