//! The desktop notification the agent shows before the portal asks the user
//! to choose a screen again (ADR-0037 D21).

use ashpd::desktop::notification::{Notification, NotificationProxy};
use ashpd::zbus::Connection;

const NOTIFICATION_ID: &str = "screen-sync-reprompt";
const TITLE: &str = "FOLO needs to know which screen to use";
const BODY: &str = "The screens changed, so the desktop is asking again which one FOLO may capture. Choose it in the dialog to carry on with screen sync.";

/// Shows the notification. A desktop with no notification service, or a
/// refusal, is not worth failing the capture for: the dialog is on screen
/// anyway.
pub async fn reprompt(connection: Connection) {
    let Ok(proxy) = NotificationProxy::with_connection(connection).await else {
        return;
    };
    let notification = Notification::new(TITLE).body(BODY);
    let _ = proxy.add_notification(NOTIFICATION_ID, notification).await;
}
