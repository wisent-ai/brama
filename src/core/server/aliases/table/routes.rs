//! Which route one alias resolves to, per capability: chat, a typed decision,
//! and the three media names.
//!
//! Each is refused for the aliases whose promise it cannot keep, because an
//! alias that resolves to the wrong shape is a render or an answer nobody can
//! pay for rather than an error a caller can read.

use super::super::{
    DECISION_ALIASES, IMAGE_ALIAS, MEDIA_ALIASES, VIDEO_ALIAS, VOICE_ALIAS,
    WISENT_EMBEDDING_ALIAS, WISENT_MODERATION_ALIAS,
};
use super::ModelAliases;

impl ModelAliases {
    /// The chat route for one alias, or nothing.
    ///
    /// Only the aliases that promise a different capability are refused here:
    /// the two typed OpenAI shapes and the two decision names, whose whole
    /// promise is a typed answer `POST /v1/chat/completions` cannot produce.
    /// This was an allowlist of five chat names, which meant an
    /// operator-defined alias passed startup validation and then served
    /// nothing: `alias_route_shape_supported` accepted it and this returned
    /// `None` for it, so the alias existed and was permanently unroutable.
    pub(in crate::core::server) fn chat_route(&self, alias: &str) -> Option<String> {
        if matches!(alias, WISENT_EMBEDDING_ALIAS | WISENT_MODERATION_ALIAS)
            || DECISION_ALIASES.contains(&alias)
            || MEDIA_ALIASES.contains(&alias)
        {
            return None;
        }
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::resolve(path, alias) {
                Ok(Some(route)) => return Self::serviceable(alias, route),
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return None;
                }
            }
        }
        self.routes
            .get(alias)
            .cloned()
            .and_then(|route| Self::serviceable(alias, route))
    }

    /// The route one decision alias resolves to, or nothing.
    ///
    /// The mirror of [`Self::chat_route`]: only the two decision names
    /// resolve here, so a caller cannot reach `POST /v1/decisions` through a
    /// chat alias any more than it can reach chat through a decision alias.
    pub(in crate::core::server) fn decision_route(&self, alias: &str) -> Option<String> {
        if !DECISION_ALIASES.contains(&alias) {
            return None;
        }
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::resolve(path, alias) {
                Ok(Some(route)) => return Self::serviceable(alias, route),
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return None;
                }
            }
        }
        self.routes
            .get(alias)
            .cloned()
            .and_then(|route| Self::serviceable(alias, route))
    }

    /// The route one media alias resolves to, or nothing.
    ///
    /// The same mirror as the decision route, in the three media shapes:
    /// `image-model` resolves here and on `POST /v1/images/generations`
    /// alone, `video-model` on `POST /v1/videos`, `voice-model` on
    /// `POST /v1/audio/speech`, and none of them on chat. Asking for the
    /// wrong one is a refusal rather than a render nobody can pay for.
    pub(in crate::core::server) fn image_route(&self, alias: &str) -> Option<String> {
        if alias != IMAGE_ALIAS {
            return None;
        }
        self.media_route(alias)
    }

    pub(in crate::core::server) fn video_route(&self, alias: &str) -> Option<String> {
        if alias != VIDEO_ALIAS {
            return None;
        }
        self.media_route(alias)
    }

    pub(in crate::core::server) fn voice_route(&self, alias: &str) -> Option<String> {
        if alias != VOICE_ALIAS {
            return None;
        }
        self.media_route(alias)
    }

    fn media_route(&self, alias: &str) -> Option<String> {
        if let Some(path) = self.routes_file.as_deref() {
            match crate::core::inference_routes::resolve(path, alias) {
                Ok(Some(route)) => return Self::serviceable(alias, route),
                Ok(None) => {}
                Err(error) => {
                    warn!(event = "inference_routes_invalid", %error);
                    return None;
                }
            }
        }
        self.routes
            .get(alias)
            .cloned()
            .and_then(|route| Self::serviceable(alias, route))
    }
}
