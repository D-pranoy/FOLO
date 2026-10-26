//! Test doubles for the tests of the provider crates and the node: a provider that records what
//! it is asked and can be made to fail, and a kind that makes it.
//!
//! Times are Tokio's [`Instant`], so tests that pause the clock see exact intervals.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use folo_protocol::generated::{
    DiscoveredDevice, ProviderInstance, ProviderInstanceKind, ProviderSecrets,
};
use serde_json::Value;
use tokio::time::Instant;

use crate::error::{AdminError, ProviderError};
use crate::traits::{AdminMethod, ElementOutput, Provider, ProviderKind};
use crate::types::{BoxFuture, DeliveryPolicy, DescribedTarget, ElementFrame, TargetOutput};

/// One call a [`RecordingProvider`] saw.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Connect,
    Describe,
    Disconnect,
    Start(String),
    Send { at: Instant, frame: ElementFrame },
    Release(String),
    Admin { action: String, method: AdminMethod },
}

#[derive(Default)]
struct State {
    events: Vec<Event>,
    described: Vec<DescribedTarget>,
    connect_failures: u32,
    fail_sends: bool,
    fail_starts: bool,
    admin: HashMap<String, Value>,
}

/// A provider that records every call and answers with what it was told to.
pub struct RecordingProvider {
    state: Mutex<State>,
    policy: DeliveryPolicy,
}

impl Default for RecordingProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingProvider {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            policy: DeliveryPolicy::default(),
        }
    }

    /// With the policy the sender should use.
    #[must_use]
    pub fn with_policy(mut self, policy: DeliveryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// With the targets `describe` answers.
    #[must_use]
    pub fn with_described(self, targets: Vec<DescribedTarget>) -> Self {
        self.lock().described = targets;
        self
    }

    /// With the answer to an administration action, for reads and changes alike.
    #[must_use]
    pub fn with_admin(self, action: &str, answer: Value) -> Self {
        self.lock().admin.insert(action.to_owned(), answer);
        self
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn record(&self, event: Event) {
        self.lock().events.push(event);
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.lock().events.clone()
    }

    /// The frames sent so far, with when each was.
    #[must_use]
    pub fn sends(&self) -> Vec<(Instant, ElementFrame)> {
        self.lock()
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Send { at, frame } => Some((*at, frame.clone())),
                _ => None,
            })
            .collect()
    }

    /// The next `n` calls to `connect` fail.
    pub fn fail_connects(&self, n: u32) {
        self.lock().connect_failures = n;
    }

    /// Whether `send` fails.
    pub fn set_fail_sends(&self, fail: bool) {
        self.lock().fail_sends = fail;
    }

    /// Whether `start` fails.
    pub fn set_fail_starts(&self, fail: bool) {
        self.lock().fail_starts = fail;
    }
}

impl Provider for RecordingProvider {
    fn connect(&self) -> BoxFuture<'_, Result<(), ProviderError>> {
        Box::pin(async move {
            let mut state = self.lock();
            state.events.push(Event::Connect);
            if state.connect_failures > 0 {
                state.connect_failures -= 1;
                return Err(ProviderError::Unreachable(
                    "the recording provider is down".to_owned(),
                ));
            }
            Ok(())
        })
    }

    fn describe(&self) -> BoxFuture<'_, Result<Vec<DescribedTarget>, ProviderError>> {
        Box::pin(async move {
            self.record(Event::Describe);
            Ok(self.lock().described.clone())
        })
    }

    fn disconnect(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.record(Event::Disconnect) })
    }

    fn admin(
        &self,
        action: &str,
        method: AdminMethod,
        _body: Value,
    ) -> BoxFuture<'_, Result<Value, AdminError>> {
        let action = action.to_owned();
        Box::pin(async move {
            let mut state = self.lock();
            state.events.push(Event::Admin {
                action: action.clone(),
                method,
            });
            state
                .admin
                .get(&action)
                .cloned()
                .ok_or(AdminError::UnknownAction(action))
        })
    }

    fn delivery_policy(&self) -> DeliveryPolicy {
        self.policy
    }

    fn element_output(&self) -> Option<&dyn ElementOutput> {
        Some(self)
    }
}

impl ElementOutput for RecordingProvider {
    fn start(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move {
            let mut state = self.lock();
            state.events.push(Event::Start(target));
            if state.fail_starts {
                return Err(ProviderError::Refused(
                    "the recording provider won't start".to_owned(),
                ));
            }
            Ok(())
        })
    }

    fn send<'a>(&'a self, frame: &'a ElementFrame) -> BoxFuture<'a, Result<(), ProviderError>> {
        Box::pin(async move {
            let mut state = self.lock();
            if state.fail_sends {
                return Err(ProviderError::Unreachable(
                    "the recording provider's send failed".to_owned(),
                ));
            }
            state.events.push(Event::Send {
                at: Instant::now(),
                frame: frame.clone(),
            });
            Ok(())
        })
    }

    /// Plain 8-bit levels, truncated.
    fn wire_key(&self, output: &TargetOutput) -> Vec<u8> {
        output.colors.to_bytes()
    }

    fn release(&self, target: &str) -> BoxFuture<'_, Result<(), ProviderError>> {
        let target = target.to_owned();
        Box::pin(async move {
            self.record(Event::Release(target));
            Ok(())
        })
    }
}

/// A kind that makes [`RecordingProvider`]s, and discovers the devices it was given.
pub struct RecordingKind {
    kind: ProviderInstanceKind,
    devices: Vec<DiscoveredDevice>,
    fail_discovery: bool,
    created: Mutex<Vec<Arc<RecordingProvider>>>,
}

impl RecordingKind {
    #[must_use]
    pub fn new(kind: ProviderInstanceKind, devices: Vec<DiscoveredDevice>) -> Self {
        Self {
            kind,
            devices,
            fail_discovery: false,
            created: Mutex::new(Vec::new()),
        }
    }

    /// Makes discovery fail.
    #[must_use]
    pub fn failing(mut self) -> Self {
        self.fail_discovery = true;
        self
    }

    /// The providers made so far, to look at what they recorded.
    #[must_use]
    pub fn created(&self) -> Vec<Arc<RecordingProvider>> {
        self.created
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl ProviderKind for RecordingKind {
    fn kind(&self) -> ProviderInstanceKind {
        self.kind
    }

    fn discover(&self) -> BoxFuture<'_, Result<Vec<DiscoveredDevice>, ProviderError>> {
        Box::pin(async move {
            if self.fail_discovery {
                return Err(ProviderError::Failed("discovery failed".to_owned()));
            }
            Ok(self.devices.clone())
        })
    }

    fn create(
        &self,
        _instance: &ProviderInstance,
        _secrets: Option<&ProviderSecrets>,
    ) -> Result<Arc<dyn Provider>, ProviderError> {
        let provider = Arc::new(RecordingProvider::new());
        self.created
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Arc::clone(&provider));
        Ok(provider)
    }
}
