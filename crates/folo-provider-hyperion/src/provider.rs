//! The Hyperion provider: the control connection, describe, and the element output.
//!
//! A Hyperion server runs instances, and a connection drives the one it has selected. The
//! provider keeps one connection (a [`Link`]) per running instance, each switched to its
//! instance, because [ADR-0011](../../docs/decisions/ADR-0011-hyperion-provider.md) D2 makes each
//! instance a target and the sender drives targets side by side. Target ids are the instance
//! numbers (`"0"`, `"1"`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use folo_protocol::generated::{
    Address, CapabilityDescriptor, CapabilityDescriptorAddressing, CapabilityDescriptorColorModel,
    CapabilityDescriptorDelivery, CapabilityLimits, LineGeometry, TargetKind,
};
use folo_provider::{
    AdminError, AdminMethod, BoxFuture, DeliveryPolicy, DescribedLayout, DescribedTarget,
    ElementColors, ElementFrame, ElementOutput, Provider, ProviderError, RasterOutput,
    TargetOutput,
};
use serde_json::{Value, json};

use crate::layout::{default_tv_plane, place_on_plane};
use crate::link::Link;
use crate::serverinfo::{InstanceInfo, ServerInfo};
use crate::settings::HyperionConfig;
use crate::wire;

/// How often the sender describes again to watch the LED count: as the legacy layout watcher
/// polls. A node builds the instance's `SenderConfig` with this as `describe_every`.
pub const DESCRIBE_EVERY: Duration = Duration::from_secs(5);

/// Hyperion keeps the colours 2 s after the last frame, so an unchanged frame goes out again
/// after at most this long (`docs/architecture/providers.md`, the sender).
pub const KEEPALIVE: Duration = Duration::from_millis(500);

/// The most positions a line geometry holds (`docs/specs/config-model.md`).
const MAX_LAYOUT_POSITIONS: usize = 2000;

/// The most LEDs an address range holds.
const MAX_ELEMENTS: usize = 65535;

pub(crate) struct State {
    /// A connection per running instance, each on its instance.
    pub links: BTreeMap<u32, Arc<Link>>,
    pub instances: Vec<InstanceInfo>,
    /// The instance administration applies to unless a request names another, and the one the
    /// first connection is on.
    pub selected: u32,
    /// The token to log in with: the configured one, or one an administrator tried and
    /// Hyperion accepted. It is kept in memory only.
    pub token: Option<String>,
}

pub struct HyperionProvider {
    pub(crate) config: HyperionConfig,
    pub(crate) state: Mutex<State>,
}

fn not_connected() -> ProviderError {
    ProviderError::Unreachable("not connected to Hyperion".to_owned())
}

impl HyperionProvider {
    #[must_use]
    pub fn new(config: HyperionConfig) -> Self {
        let token = config.token.clone();
        Self {
            config,
            state: Mutex::new(State {
                links: BTreeMap::new(),
                instances: Vec::new(),
                selected: 0,
                token,
            }),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn link(&self, instance: u32) -> Option<Arc<Link>> {
        self.lock().links.get(&instance).cloned()
    }

    /// Opens a connection, logs in when there is a token, and switches it to `instance`.
    pub(crate) async fn open_link(&self, instance: Option<u32>) -> Result<Link, ProviderError> {
        let link = Link::open(&self.config.host, self.config.port).await?;
        let token = self.lock().token.clone();
        if let Some(token) = token {
            login(&link, &token).await?;
        }
        if let Some(instance) = instance {
            switch_to(&link, instance).await?;
        }
        Ok(link)
    }

    /// The connection of a running instance, opened when `describe` hasn't yet.
    pub(crate) async fn link_or_open(&self, instance: u32) -> Result<Arc<Link>, ProviderError> {
        if let Some(link) = self.link(instance)
            && link.is_open()
        {
            return Ok(link);
        }
        let known = self
            .lock()
            .instances
            .iter()
            .any(|i| i.number == instance && i.running);
        if !known {
            return Err(ProviderError::Refused(format!(
                "Hyperion has no running instance {instance}"
            )));
        }
        let opened = Arc::new(self.open_link(Some(instance)).await?);
        // Another request may have opened it meanwhile: the first stays
        let mut state = self.lock();
        match state.links.get(&instance) {
            Some(first) if first.is_open() => Ok(Arc::clone(first)),
            _ => {
                state.links.insert(instance, Arc::clone(&opened));
                Ok(opened)
            }
        }
    }

    /// `serverinfo` of the instance a link is on.
    pub(crate) async fn server_info(link: &Link) -> Result<ServerInfo, ProviderError> {
        ServerInfo::parse(&link.request(json!({"command": "serverinfo"})).await?)
    }

    fn target(number: u32, name: &str, info: &ServerInfo) -> Option<DescribedTarget> {
        let elements = info.leds.len();
        let count = std::num::NonZeroU64::new(elements as u64)?;
        if elements > MAX_ELEMENTS {
            log::warn!("Hyperion instance {number} has too many LEDs ({elements}), skipping it");
            return None;
        }
        let layouts = if elements <= MAX_LAYOUT_POSITIONS {
            let geometry = LineGeometry {
                count,
                kind: json!("line"),
                path: Vec::new(),
                positions: place_on_plane(&info.leds, &default_tv_plane()),
            };
            vec![DescribedLayout {
                name: "Hyperion LED layout".to_owned(),
                geometry: geometry.into(),
            }]
        } else {
            Vec::new()
        };
        Some(DescribedTarget {
            id: number.to_string(),
            name: if name.is_empty() {
                format!("Hyperion {number}")
            } else {
                name.to_owned()
            },
            kind: TargetKind::Line,
            address: Address {
                channels: Vec::new(),
                count,
                start: 0,
            },
            elements: u32::try_from(elements).ok()?,
            capabilities: CapabilityDescriptor {
                addressing: if elements == 1 {
                    CapabilityDescriptorAddressing::Single
                } else {
                    CapabilityDescriptorAddressing::Pixel
                },
                color_model: CapabilityDescriptorColorModel::Rgb,
                delivery: CapabilityDescriptorDelivery::Streaming,
                limits: CapabilityLimits::default(),
            },
            layouts,
        })
    }

    async fn do_connect(&self) -> Result<(), ProviderError> {
        let old: Vec<_> = std::mem::take(&mut self.lock().links)
            .into_values()
            .collect();
        drop(old);
        let link = Arc::new(self.open_link(None).await?);
        let info = Self::server_info(&link).await?;
        let previous = self.lock().selected;
        let selected = if info.instances.is_empty() {
            0
        } else {
            let running = info.running();
            let Some(first) = running.first() else {
                return Err(ProviderError::Failed(
                    "Hyperion has no running instance".to_owned(),
                ));
            };
            let keep = running.iter().any(|i| i.number == previous);
            let chosen = if keep { previous } else { first.number };
            switch_to(&link, chosen).await?;
            chosen
        };
        let mut state = self.lock();
        state.instances = info.instances;
        state.selected = selected;
        state.links.insert(selected, link);
        Ok(())
    }

    async fn do_describe(&self) -> Result<Vec<DescribedTarget>, ProviderError> {
        let selected = self.lock().selected;
        if self.lock().links.is_empty() {
            return Err(not_connected());
        }
        let control = self.link_or_open(selected).await?;
        let own = Self::server_info(&control).await?;
        let mut instances = own.instances.clone();
        if instances.is_empty() {
            instances.push(InstanceInfo {
                number: selected,
                name: String::new(),
                running: true,
            });
        }
        {
            let mut state = self.lock();
            state.instances.clone_from(&instances);
            let running: Vec<u32> = instances
                .iter()
                .filter(|i| i.running)
                .map(|i| i.number)
                .collect();
            state.links.retain(|n, _| running.contains(n));
        }
        let mut targets = Vec::new();
        for instance in instances.iter().filter(|i| i.running) {
            let info = if instance.number == selected {
                own.clone()
            } else {
                let link = self.link_or_open(instance.number).await?;
                if !link.is_open() {
                    return Err(ProviderError::Unreachable(
                        "Hyperion closed the connection".to_owned(),
                    ));
                }
                Self::server_info(&link).await?
            };
            targets.extend(Self::target(instance.number, &instance.name, &info));
        }
        Ok(targets)
    }

    fn rgb_bytes(output: &TargetOutput) -> Result<Vec<u8>, ProviderError> {
        match &output.colors {
            colors @ ElementColors::Rgb(_) => Ok(colors.to_bytes()),
            other => Err(ProviderError::Failed(format!(
                "Hyperion takes RGB, target {} has {:?} colours",
                output.target,
                other.color_model()
            ))),
        }
    }

    fn instance_of(target: &str) -> Option<u32> {
        target.parse().ok()
    }
}

async fn login(link: &Link, token: &str) -> Result<(), ProviderError> {
    // Hyperion's schema rejects a shorter token, and its login handler never answers one
    if token.chars().count() < wire::MIN_TOKEN_CHARS {
        return Err(ProviderError::Refused(format!(
            "an API token has at least {} characters",
            wire::MIN_TOKEN_CHARS
        )));
    }
    link.request(json!({"command": "authorize", "subcommand": "login", "token": token}))
        .await
        .map(drop)
}

async fn switch_to(link: &Link, instance: u32) -> Result<(), ProviderError> {
    link.request(json!({"command": "instance", "subcommand": "switchTo", "instance": instance}))
        .await
        .map(drop)
}

impl Provider for HyperionProvider {
    fn connect(&self) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(self.do_connect())
    }

    fn describe(&self) -> BoxFuture<'_, Result<Vec<DescribedTarget>, ProviderError>> {
        Box::pin(self.do_describe())
    }

    fn disconnect(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let links: Vec<_> = std::mem::take(&mut self.lock().links)
                .into_values()
                .collect();
            for link in links {
                link.close().await;
            }
        })
    }

    fn admin(
        &self,
        action: &str,
        method: AdminMethod,
        body: Value,
    ) -> BoxFuture<'_, Result<Value, AdminError>> {
        let action = action.to_owned();
        Box::pin(async move { self.run_admin(&action, method, &body).await })
    }

    fn delivery_policy(&self) -> DeliveryPolicy {
        DeliveryPolicy {
            keepalive: KEEPALIVE,
            ..DeliveryPolicy::default()
        }
    }

    fn element_output(&self) -> Option<&dyn ElementOutput> {
        Some(self)
    }

    fn raster_output(&self) -> Option<&dyn RasterOutput> {
        None
    }
}

impl ElementOutput for HyperionProvider {
    /// Hyperion needs nothing started; this only checks that the target is an instance it runs.
    fn start(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move {
            let known = Self::instance_of(&target).and_then(|n| self.link(n));
            let Some(link) = known else {
                if self.lock().links.is_empty() {
                    return Err(not_connected());
                }
                return Err(ProviderError::Refused(format!(
                    "Hyperion has no running instance {target}"
                )));
            };
            if link.is_open() {
                Ok(())
            } else {
                Err(ProviderError::Unreachable(
                    "Hyperion closed the connection".to_owned(),
                ))
            }
        })
    }

    fn send<'a>(&'a self, frame: &'a ElementFrame) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move {
            for output in &frame.targets {
                let rgb = Self::rgb_bytes(output)?;
                // The legacy sender sends nothing for an empty frame
                if rgb.is_empty() {
                    continue;
                }
                let link = Self::instance_of(&output.target)
                    .and_then(|n| self.link(n))
                    .ok_or_else(|| {
                        ProviderError::Unreachable(format!(
                            "no connection to Hyperion instance {}",
                            output.target
                        ))
                    })?;
                let command = wire::color_line(&rgb, self.config.priority, &self.config.origin);
                link.send_line(&command).await?;
            }
            Ok(())
        })
    }

    /// The levels as bytes, which are what the `color` command carries.
    fn wire_key(&self, output: &TargetOutput) -> Vec<u8> {
        output.colors.to_bytes()
    }

    /// Clears FOLO's priority on the target's instance. A target that isn't running has no
    /// connection, and nothing to clear.
    fn release(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move {
            let Some(link) = Self::instance_of(&target).and_then(|n| self.link(n)) else {
                return Ok(());
            };
            link.send_line(&wire::clear_line(self.config.priority))
                .await
        })
    }
}
