//! A target's effective capabilities.

use folo_protocol::generated::{
    Capabilities, CapabilityDescriptor, CapabilityDescriptorAddressing,
    CapabilityDescriptorColorModel, CapabilityDescriptorDelivery, CapabilityOverridesAddressing,
    CapabilityOverridesColorModel, CapabilityOverridesDelivery,
};

fn addressing(value: CapabilityOverridesAddressing) -> CapabilityDescriptorAddressing {
    match value {
        CapabilityOverridesAddressing::Single => CapabilityDescriptorAddressing::Single,
        CapabilityOverridesAddressing::Zoned => CapabilityDescriptorAddressing::Zoned,
        CapabilityOverridesAddressing::Pixel => CapabilityDescriptorAddressing::Pixel,
    }
}

fn color_model(value: CapabilityOverridesColorModel) -> CapabilityDescriptorColorModel {
    match value {
        CapabilityOverridesColorModel::Onoff => CapabilityDescriptorColorModel::Onoff,
        CapabilityOverridesColorModel::Dimmer => CapabilityDescriptorColorModel::Dimmer,
        CapabilityOverridesColorModel::Cct => CapabilityDescriptorColorModel::Cct,
        CapabilityOverridesColorModel::Rgb => CapabilityDescriptorColorModel::Rgb,
        CapabilityOverridesColorModel::Rgbw => CapabilityDescriptorColorModel::Rgbw,
        CapabilityOverridesColorModel::Rgbcct => CapabilityDescriptorColorModel::Rgbcct,
        CapabilityOverridesColorModel::Xy => CapabilityDescriptorColorModel::Xy,
    }
}

fn delivery(value: CapabilityOverridesDelivery) -> CapabilityDescriptorDelivery {
    match value {
        CapabilityOverridesDelivery::Streaming => CapabilityDescriptorDelivery::Streaming,
        CapabilityOverridesDelivery::State => CapabilityDescriptorDelivery::State,
    }
}

/// What a target can do: the descriptor its provider discovered with the user's overrides on top.
/// An override of `limits` replaces the discovered limits as a whole, because the contract
/// carries it as one object whose missing fields have already taken their defaults.
#[must_use]
pub fn effective_capabilities(capabilities: &Capabilities) -> CapabilityDescriptor {
    let mut out = capabilities.discovered.clone();
    let overrides = &capabilities.overrides;
    if let Some(value) = overrides.addressing {
        out.addressing = addressing(value);
    }
    if let Some(value) = overrides.color_model {
        out.color_model = color_model(value);
    }
    if let Some(value) = overrides.delivery {
        out.delivery = delivery(value);
    }
    if let Some(limits) = &overrides.limits {
        out.limits = limits.clone();
    }
    out
}
