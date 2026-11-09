//! The public values compiled into the app (plan: Supabase URL, anon key,
//! Worker URL). None is a secret: the anon key only identifies the project.

use super::AskError;

const SUPABASE_URL: &str = "https://aklgszmriarobrlkrzcd.supabase.co";
/// The publishable key (`sb_publishable_...`) or the legacy anon key.
const SUPABASE_ANON_KEY: &str = "sb_publishable_AVnDnk6l6GlRMxoTSLR4iQ_SblqIzxu";
/// The deployed Worker, `https://magi-ask.<subdomain>.workers.dev`.
const WORKER_URL: &str = "https://magi-ask.juanparracsfn.workers.dev";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub supabase_url: String,
    pub anon_key: String,
    pub worker_url: String,
}

impl Endpoints {
    pub fn compiled() -> Result<Self, AskError> {
        if SUPABASE_ANON_KEY.is_empty() || WORKER_URL.is_empty() {
            return Err(AskError::NotConfigured);
        }
        Ok(Self {
            supabase_url: SUPABASE_URL.trim_end_matches('/').to_string(),
            anon_key: SUPABASE_ANON_KEY.to_string(),
            worker_url: WORKER_URL.trim_end_matches('/').to_string(),
        })
    }
}
