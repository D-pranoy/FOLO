# Home Assistant

Status: Accepted · Implementation: Not started (readiness in v1; integration after v1)

Decided in [ADR-0022](../decisions/ADR-0022-home-assistant-integration-strategy.md).

## Goal

Home Assistant should control FOLO's **effects and effect controls**: zones, scenes, parameters, sources and updates. HA should not supply lighting pixel data. The integration must be setup-agnostic and optional.

## Options considered

| Option | Effect control | Pros | Cons |
|---|---|---|---|
| **Native integration, WLED-style** | Full | The proven pattern: WLED's core integration is Platinum-rated, and Hyperion and Hue use the same model. Push updates, automations, voice, dashboards. Pixels never pass through HA. | A Python integration and a PyPI client library to maintain, tracking HA's monthly releases |
| MQTT discovery | Good: light effects, select, number, update, event | No HA-side code; also works with openHAB and Node-RED | Needs a broker; no config flow, approval flow or action responses |
| HA context as effect inputs | Inputs only | Context-aware lighting (sun, media players, doorbells) | A bindings UI |
| Matter bridge | Basic only, no effects | Reaches Apple, Google, Alexa | Loses effects; complex commissioning |
| HA as a pixel source | Poor | Simple idea | HA records every state change, and its WebSocket fan-out drops slow clients (4096 pending messages). **Avoid.** |
| HA lights as FOLO outputs | Slow colour only | Reaches any HA-controlled bulb | A few updates per second, and Zigbee mesh load. Ambient use only. |
| FOLO as an HA app (formerly add-on) | Hosting only | One-click setup for HA OS users | Another release channel; capture inside HA OS is limited |

## Chosen approach

1. **Phase 0, in v1: HA readiness.**
   - Stable controller and zone ids.
   - The `_folo._tcp` TXT record with `id` and `api`.
   - Full-state WebSocket push.
   - An effect catalog with parameter schemas.
   - Pairing request and approval endpoints with scoped client keys.
   - OTA summary and release URL.
   - An **async Python client library** (aiohttp, accepts an injected session, typed), published to PyPI from CI on tagged releases.
2. **Phase 1:** a HACS integration (`integration_type: hub`, `local_push`, zeroconf discovery):
   - one HA device per FOLO zone
   - a light per zone, whose `effect_list` lists the scenes and includes `EFFECT_OFF`
   - `number` entities for intensity, speed and sensitivity
   - `select` entities for palette, scene and source
   - `switch` for sync and screen sync
   - `sensor` for status, frame rate and outputs
   - `button` for identify and restart
   - `update` for OTA
   - actions `folo.apply_effect` and `folo.get_effect_catalog`
   - Pairing follows Hyperion's pattern: HA shows a request id, a FOLO admin approves, and the key is issued. Reauth and diagnostics are included.
3. **Phase 2:** HA context as effect inputs. The integration forwards the entities and events the user picks to FOLO. A direct WebSocket subscription is an advanced alternative.
4. **Phase 3:** a Home Assistant app for running the controller inside HA OS: ingress, host networking, Supervisor discovery.
5. **Phase 4:** a core submission. First the light platform alone (Bronze), then the other platforms and Silver/Gold in follow-up PRs.

## Mapping rules

- **Naming:** HA's "zone" means a geographic area, so FOLO zones appear as HA **devices**, which users can put in HA areas.
- **OTA:** in HA, pressing Install is the approval. Update entities can install only with an `admin`-scoped client key; with `control` scope they are display-only.
- **Rate:** beats and audio events are not HA events. HA events are only for low-rate, discrete changes such as "scene changed" or "update needs approval".

## Sources

- [HA WLED](https://www.home-assistant.io/integrations/wled/), [HA Hyperion](https://www.home-assistant.io/integrations/hyperion/), [HA Hue](https://www.home-assistant.io/integrations/hue/)
- [HA light entity](https://developers.home-assistant.io/docs/core/entity/light/), [HA update entity](https://developers.home-assistant.io/docs/core/entity/update/)
- [HA quality scale](https://developers.home-assistant.io/docs/core/integration-quality-scale/), [HA API library rules](https://developers.home-assistant.io/docs/api_lib_index/)
- [HACS publishing](https://www.hacs.xyz/docs/publish/integration/), [HA MQTT](https://www.home-assistant.io/integrations/mqtt/), [HA app configuration](https://developers.home-assistant.io/docs/add-ons/configuration/)
- [HA WebSocket API](https://developers.home-assistant.io/docs/api/websocket/), [Hyperion HA LED device PR](https://github.com/hyperion-project/hyperion.ng/pull/1763)
