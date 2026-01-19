use crate::{PlatformKeyboardLayout, SharedString};

#[derive(Clone)]
pub(crate) struct AndroidKeyboardLayout {
    name: SharedString,
}

impl PlatformKeyboardLayout for AndroidKeyboardLayout {
    fn id(&self) -> &str {
        &self.name
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl AndroidKeyboardLayout {
    pub(crate) fn new(name: SharedString) -> Self {
        Self { name }
    }
}
