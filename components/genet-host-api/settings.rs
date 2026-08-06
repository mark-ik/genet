/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Product-neutral settings projection contracts.
//!
//! This module describes settings for a host surface. It deliberately owns
//! neither persistence nor a registry of all settings. The configured product
//! owns the typed value and storage; a provider adapts that owner to the
//! projection surface.

use crate::tile::SettingsRef;

/// The thing whose behavior the setting configures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingScope {
    Process,
    Device,
    Persona,
    Application,
    SessionDataspace,
    ArtifactEntity,
    GovernedPlace,
}

/// How a setting may move beyond its current owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingMovement {
    LocalOnly,
    PersonaSynced,
    TravelsWithArtifact,
    GovernedReplication,
}

/// When a changed setting takes effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingMutability {
    StartupOnly,
    Live,
    RestartRequired,
}

/// What kind of value may cross the projection boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingSecurity {
    Ordinary,
    Private,
    SecretReference,
}

/// A typed value shown by a setting control.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingValue {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    Text(String),
}

/// One selectable value for a [`SettingControl::Choice`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingOption {
    pub value: String,
    pub label: String,
}

/// The control vocabulary a renderer may use for an editable setting.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingControl {
    Toggle,
    Choice {
        options: Vec<SettingOption>,
    },
    Number {
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
    },
    Text,
}

/// A provider-described setting row.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingSpec {
    pub id: String,
    pub label: String,
    pub scope: SettingScope,
    pub movement: SettingMovement,
    pub mutability: SettingMutability,
    pub security: SettingSecurity,
    pub control: SettingControl,
    pub value: SettingValue,
}

/// The shared provider-to-view-model step.
///
/// Products still render controls with their own retained state, but they use
/// this same resolution boundary before doing so. Keeping the projection
/// model here avoids a second provider lookup convention without turning the
/// host contract into a widget crate.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsProjection {
    pub reference: SettingsRef,
    pub specs: Vec<SettingSpec>,
}

impl SettingsProjection {
    pub fn resolve(
        provider: &dyn SettingsProvider,
        reference: &SettingsRef,
    ) -> Result<Self, SettingsError> {
        Ok(Self {
            reference: reference.clone(),
            specs: provider.describe(reference)?,
        })
    }
}

/// Failure while resolving a settings page or applying one of its writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsError {
    UnsupportedReference(SettingsRef),
    UnknownSetting(String),
    InvalidValue { setting_id: String, message: String },
    Storage(String),
}

/// A host-side adapter for one or more settings namespaces.
///
/// The trait is intentionally object-safe and synchronous. A host can keep
/// asynchronous persistence, restart scheduling, and permission checks behind
/// its implementation without making the projection contract a store or a
/// runtime-wide registry.
pub trait SettingsProvider {
    /// Describe the settings addressed by `reference`.
    fn describe(&self, reference: &SettingsRef) -> Result<Vec<SettingSpec>, SettingsError>;

    /// Apply one typed user write to the owner of the setting.
    fn apply(
        &mut self,
        reference: &SettingsRef,
        setting_id: &str,
        value: SettingValue,
    ) -> Result<(), SettingsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = "pelt/appearance";

    struct ToggleProvider {
        enabled: bool,
    }

    impl SettingsProvider for ToggleProvider {
        fn describe(&self, reference: &SettingsRef) -> Result<Vec<SettingSpec>, SettingsError> {
            if reference.0 != REFERENCE {
                return Err(SettingsError::UnsupportedReference(reference.clone()));
            }
            Ok(vec![SettingSpec {
                id: "chrome.enabled".into(),
                label: "Chrome enabled".into(),
                scope: SettingScope::Application,
                movement: SettingMovement::LocalOnly,
                mutability: SettingMutability::Live,
                security: SettingSecurity::Ordinary,
                control: SettingControl::Toggle,
                value: SettingValue::Boolean(self.enabled),
            }])
        }

        fn apply(
            &mut self,
            reference: &SettingsRef,
            setting_id: &str,
            value: SettingValue,
        ) -> Result<(), SettingsError> {
            if reference.0 != REFERENCE {
                return Err(SettingsError::UnsupportedReference(reference.clone()));
            }
            if setting_id != "chrome.enabled" {
                return Err(SettingsError::UnknownSetting(setting_id.into()));
            }
            match value {
                SettingValue::Boolean(enabled) => {
                    self.enabled = enabled;
                    Ok(())
                },
                other => Err(SettingsError::InvalidValue {
                    setting_id: setting_id.into(),
                    message: format!("expected Boolean, got {other:?}"),
                }),
            }
        }
    }

    #[test]
    fn provider_describes_axes_control_and_value() {
        let provider = ToggleProvider { enabled: true };
        let specs = provider.describe(&SettingsRef(REFERENCE.into())).unwrap();
        assert_eq!(specs[0].scope, SettingScope::Application);
        assert_eq!(specs[0].control, SettingControl::Toggle);
        assert_eq!(specs[0].value, SettingValue::Boolean(true));
    }

    #[test]
    fn provider_applies_a_typed_write_and_rejects_wrong_types() {
        let reference = SettingsRef(REFERENCE.into());
        let mut provider = ToggleProvider { enabled: false };
        provider
            .apply(&reference, "chrome.enabled", SettingValue::Boolean(true))
            .unwrap();
        assert_eq!(
            provider.describe(&reference).unwrap()[0].value,
            SettingValue::Boolean(true)
        );
        assert!(matches!(
            provider.apply(
                &reference,
                "chrome.enabled",
                SettingValue::Text("yes".into())
            ),
            Err(SettingsError::InvalidValue { .. })
        ));
    }

    #[test]
    fn projection_resolves_once_for_a_renderer() {
        let provider = ToggleProvider { enabled: true };
        let reference = SettingsRef(REFERENCE.into());
        let projection = SettingsProjection::resolve(&provider, &reference).unwrap();
        assert_eq!(projection.reference, reference);
        assert_eq!(projection.specs.len(), 1);
    }
}
