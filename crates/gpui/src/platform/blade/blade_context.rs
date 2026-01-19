use anyhow::Context as _;
use blade_graphics as gpu;
use std::sync::Arc;
use util::ResultExt;

#[cfg_attr(target_os = "macos", derive(Clone))]
pub struct BladeContext {
    pub(super) gpu: Arc<gpu::Context>,
}

impl BladeContext {
    pub fn gpu_context(&self) -> &Arc<gpu::Context> {
        &self.gpu
    }

    pub fn new() -> anyhow::Result<Self> {
        let device_id_forced = match std::env::var("ZED_DEVICE_ID") {
            Ok(val) => parse_pci_id(&val)
                .context("Failed to parse device ID from `ZED_DEVICE_ID` environment variable")
                .log_err(),
            Err(std::env::VarError::NotPresent) => None,
            err => {
                err.context("Failed to read value of `ZED_DEVICE_ID` environment variable")
                    .log_err();
                None
            }
        };

        // On Android, we need to be more lenient with device selection
        #[cfg(target_os = "android")]
        log::info!("Initializing Blade graphics context for Android...");

        let gpu = Arc::new(
            unsafe {
                gpu::Context::init(gpu::ContextDesc {
                    presentation: true,
                    validation: false, // Disable validation on Android for compatibility
                    #[cfg(target_os = "android")]
                    device_id: 0, // Let Blade auto-select on Android
                    #[cfg(not(target_os = "android"))]
                    device_id: device_id_forced.unwrap_or(0),
                    ..Default::default()
                })
            }
            .map_err(|e| {
                log::error!("Failed to initialize Blade GPU context: {:?}", e);
                anyhow::anyhow!("{e:?}")
            })?,
        );

        #[cfg(target_os = "android")]
        log::info!("✓ Blade GPU context initialized successfully");

        Ok(Self { gpu })
    }

    #[allow(dead_code)]
    pub fn supports_dual_source_blending(&self) -> bool {
        self.gpu.capabilities().dual_source_blending
    }
}

fn parse_pci_id(id: &str) -> anyhow::Result<u32> {
    let mut id = id.trim();

    if id.starts_with("0x") || id.starts_with("0X") {
        id = &id[2..];
    }
    let is_hex_string = id.chars().all(|c| c.is_ascii_hexdigit());
    let is_4_chars = id.len() == 4;
    anyhow::ensure!(
        is_4_chars && is_hex_string,
        "Expected a 4 digit PCI ID in hexadecimal format"
    );

    u32::from_str_radix(id, 16).context("parsing PCI ID as hex")
}

#[cfg(test)]
mod tests {
    use super::parse_pci_id;

    #[test]
    fn test_parse_device_id() {
        assert!(parse_pci_id("0xABCD").is_ok());
        assert!(parse_pci_id("ABCD").is_ok());
        assert!(parse_pci_id("abcd").is_ok());
        assert!(parse_pci_id("1234").is_ok());
        assert!(parse_pci_id("123").is_err());
        assert_eq!(
            parse_pci_id(&format!("{:x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:X}", 0x1234)).unwrap(),
        );

        assert_eq!(
            parse_pci_id(&format!("{:#x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:#X}", 0x1234)).unwrap(),
        );
        assert_eq!(
            parse_pci_id(&format!("{:#x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:#X}", 0x1234)).unwrap(),
        );
    }
}
