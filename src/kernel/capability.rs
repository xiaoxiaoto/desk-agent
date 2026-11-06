use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Available,
    Unavailable,
    RequiresConfiguration,
    RequiresPermission,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityChannel {
    LocalRuntime,
    MacosGui,
    VisionProvider,
    EnterpriseApi,
    CustomerServiceApi,
    ArchiveApi,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub id: &'static str,
    pub status: CapabilityStatus,
    pub channel: CapabilityChannel,
    pub required_scopes: Vec<&'static str>,
    pub prerequisites: Vec<&'static str>,
    pub limitations: Vec<&'static str>,
    pub reason: Option<&'static str>,
}

pub fn initial_capabilities() -> Vec<Capability> {
    let mut capabilities = vec![
        Capability {
            id: "runtime.health",
            status: CapabilityStatus::Available,
            channel: CapabilityChannel::LocalRuntime,
            required_scopes: vec![],
            prerequisites: vec![],
            limitations: vec![],
            reason: None,
        },
        Capability {
            id: "wecom.client_status",
            status: CapabilityStatus::Unavailable,
            channel: CapabilityChannel::MacosGui,
            required_scopes: vec!["system.read"],
            prerequisites: vec!["macOS GUI adapter"],
            limitations: vec!["Not implemented; no client state is inferred."],
            reason: Some("macOS WeCom adapter has not been implemented"),
        },
        Capability {
            id: "vision.analysis",
            status: CapabilityStatus::Unavailable,
            channel: CapabilityChannel::VisionProvider,
            required_scopes: vec!["vision.read"],
            prerequisites: vec!["Configured Vision Provider"],
            limitations: vec!["No provider is configured or invoked."],
            reason: Some("Vision provider integration has not been implemented"),
        },
    ];
    capabilities.push(crate::applications::wecom::launch_capability());
    capabilities.push(crate::applications::wecom::center_capability());
    capabilities
}
