//! The traits a provider implements.

use std::sync::Arc;

use folo_protocol::generated::{
    DiscoveredDevice, ProviderInstance, ProviderInstanceKind, ProviderSecrets,
};
use serde_json::Value;

use crate::error::{AdminError, ProviderError};
use crate::types::{BoxFuture, DeliveryPolicy, DescribedTarget, ElementFrame, TargetOutput};

/// A kind of provider (`hyperion`, `wled`, `hue`): discovery, and making instances.
pub trait ProviderKind: Send + Sync {
    fn kind(&self) -> ProviderInstanceKind;

    /// Finds devices of this kind on the network. It changes no configuration.
    fn discover(&self) -> BoxFuture<'_, Result<Vec<DiscoveredDevice>, ProviderError>>;

    /// Makes the provider of an instance of this kind. Nothing connects yet: the sender does
    /// that. `secrets` are the instance's credentials, when there are any.
    ///
    /// # Errors
    /// When the instance's settings can't be used.
    fn create(
        &self,
        instance: &ProviderInstance,
        secrets: Option<&ProviderSecrets>,
    ) -> Result<Arc<dyn Provider>, ProviderError>;
}

/// Whether an administration request reads or changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdminMethod {
    /// `GET`: the current values and, for an action, what a change would change.
    Read,
    /// `POST`: makes the change.
    Change,
}

/// One instance of a provider: its control connection, what it describes, and its
/// administration. Its output is [`ElementOutput`] (or, reserved, [`RasterOutput`]).
///
/// An instance's state is kept by its sender ([`spawn_sender`](crate::spawn_sender)), which
/// decides it from how these calls end.
pub trait Provider: Send + Sync {
    /// Opens the **control connection**: reaches the device, authenticates, and leaves it ready
    /// for [`describe`](Self::describe) and administration. It starts no realtime output. Called
    /// again after a loss, so it must cope with being called on a half-open instance.
    fn connect(&self) -> BoxFuture<'_, Result<(), ProviderError>>;

    /// Asks the connected device what it has.
    fn describe(&self) -> BoxFuture<'_, Result<Vec<DescribedTarget>, ProviderError>>;

    /// Closes the control connection. Called after the targets are released, and never fails:
    /// whatever goes wrong is the provider's to log.
    fn disconnect(&self) -> BoxFuture<'_, ()>;

    /// A basic administration action ([ADR-0014](../../docs/decisions/ADR-0014-device-administration-scope.md)).
    /// A read says what a change would change.
    ///
    /// # Errors
    /// [`AdminError::UnknownAction`] for an action the kind doesn't have.
    fn admin(
        &self,
        action: &str,
        method: AdminMethod,
        body: Value,
    ) -> BoxFuture<'_, Result<Value, AdminError>> {
        let _ = (method, body);
        let action = action.to_owned();
        Box::pin(async move { Err(AdminError::UnknownAction(action)) })
    }

    /// The credentials this provider holds now, when an administration action has changed them
    /// (Hyperion's token after a login that worked). The node keeps them in `secrets.json`
    /// after each change; a kind without credentials of its own answers `None`.
    fn secrets(&self) -> Option<ProviderSecrets> {
        None
    }

    /// What the sender needs to know of this kind: keepalive and the state bucket.
    fn delivery_policy(&self) -> DeliveryPolicy;

    /// The element output, when the instance has one.
    fn element_output(&self) -> Option<&dyn ElementOutput>;

    /// The raster output. Reserved: no provider has one in v1.
    fn raster_output(&self) -> Option<&dyn RasterOutput> {
        None
    }
}

/// Output of colours per element, the only output that v1 builds.
pub trait ElementOutput: Send + Sync {
    /// Starts realtime output for a target: Hue's entertainment stream, WLED's realtime mode.
    /// Called when an active zone, or a test pattern, first drives the target, and again after
    /// the connection was lost. A provider that needs nothing returns at once.
    ///
    /// - **Per target, in turn:** starts and releases are per target and one after the other,
    ///   even when the device's realtime output is device-wide (WLED's one session, a Hue
    ///   bridge's one stream). Such a provider counts them: it begins on the first target
    ///   started and ends on the last one released, and a Hue provider that must rebuild its
    ///   area when the set changes does that on the count, not once per target.
    /// - **Failures:** `Unreachable` means the connection is the trouble, and the instance
    ///   reconnects. `Refused` or `Failed` (an unknown target, a bridge in use by another app)
    ///   is about this target only: the others carry on, and the sender tries it again later.
    ///   The sender doesn't release a target whose `start` was refused or failed, so the
    ///   provider undoes whatever it began before it returns the error.
    /// - **Cancelled or repeated:** a shutdown may cancel this call, and after a lost connection
    ///   it may be called for a target that already runs. It must cope with both, and the
    ///   target is released afterwards either way.
    fn start(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>>;

    /// Sends the colours of the targets in the frame, which holds only those to send now.
    /// Colours are in each target's colour model, on the 0–255 scale.
    fn send<'a>(&'a self, frame: &'a ElementFrame) -> BoxFuture<'a, Result<(), ProviderError>>;

    /// The bytes (or a digest of them) the provider would send for this target's colours: the
    /// frame as the device receives it, after truncation and encoding. The sender sends a
    /// streaming frame again only when its key differs from the last one sent, so two frames
    /// that the device can't tell apart must have the same key. Called once per target per tick,
    /// so it must be cheap; [`ElementColors::to_bytes`](crate::ElementColors::to_bytes) is the
    /// key of a device that takes plain 8-bit levels.
    fn wire_key(&self, output: &TargetOutput) -> Vec<u8>;

    /// Gives one target's lights back: clears the priority (Hyperion), ends realtime (WLED),
    /// closes the stream (Hue). Called when nothing drives the target any more, and for every
    /// started target when the instance stops, including one whose `start` was cancelled: it
    /// must be harmless for a target that isn't running. On a device-wide output,
    /// only the last target's release ends the output (see [`start`](Self::start)).
    fn release(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>>;
}

/// Output of whole images, for devices that take them. Reserved: it has no operations yet, and
/// an instance whose `output` is `raster` is validated but never executed in v1.
pub trait RasterOutput: Send + Sync {}
