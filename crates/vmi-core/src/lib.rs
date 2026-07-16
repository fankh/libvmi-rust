use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex, RwLock},
};

use vmi_arch_api::{AddressTranslator, Translation};
use vmi_driver_api::{read_bytes, read_scalar, write_bytes, Connector, Session};
use vmi_types::{
    AttachRequest, ByteOrder, Gpa, Gva, ProviderDescriptor, Result, Scalar, TranslationRoot,
    VmiError,
};

fn page_remaining(address: u64) -> Result<usize> {
    let offset = u16::try_from(address & 0xfff)
        .map_err(|_| vmi_types::VmiError::Backend("page offset overflow".into()))?;
    4096usize
        .checked_sub(usize::from(offset))
        .ok_or_else(|| VmiError::Backend("page offset exceeds page size".into()))
}

#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<BTreeMap<String, Arc<dyn Connector>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, connector: Arc<dyn Connector>) -> Result<()> {
        let id = try_clone_string(&connector.descriptor().id, "provider ID")?;
        if id.is_empty() {
            return Err(VmiError::Backend("provider ID must not be empty".into()));
        }
        let mut providers = self.providers.write().map_err(registry_poisoned)?;
        if providers.contains_key(&id) {
            return Err(VmiError::Backend(format!(
                "provider {id} is already registered"
            )));
        }
        providers.insert(id, connector);
        Ok(())
    }

    pub fn unregister(&self, id: &str) -> Result<Arc<dyn Connector>> {
        self.providers
            .write()
            .map_err(registry_poisoned)?
            .remove(id)
            .ok_or_else(|| VmiError::Backend(format!("provider {id} is not registered")))
    }

    pub fn connector(&self, id: &str) -> Result<Arc<dyn Connector>> {
        self.providers
            .read()
            .map_err(registry_poisoned)?
            .get(id)
            .cloned()
            .ok_or_else(|| VmiError::Backend(format!("provider {id} is not registered")))
    }

    pub fn descriptors(&self) -> Result<Vec<ProviderDescriptor>> {
        let providers = self.providers.read().map_err(registry_poisoned)?;
        let mut connectors = Vec::new();
        connectors
            .try_reserve_exact(providers.len())
            .map_err(|error| {
                VmiError::Backend(format!(
                    "failed to allocate provider connector snapshot: {error}"
                ))
            })?;
        connectors.extend(providers.values().cloned());
        drop(providers);
        let mut descriptors = Vec::new();
        descriptors
            .try_reserve_exact(connectors.len())
            .map_err(|error| {
                VmiError::Backend(format!(
                    "failed to allocate provider descriptor list: {error}"
                ))
            })?;
        for connector in &connectors {
            descriptors.push(try_clone_descriptor(connector.descriptor())?);
        }
        Ok(descriptors)
    }

    pub fn attach(&self, id: &str, request: AttachRequest) -> Result<VmiSession> {
        let connector = self.connector(id)?;
        VmiSession::attach(connector.as_ref(), request)
    }

    pub fn len(&self) -> Result<usize> {
        Ok(self.providers.read().map_err(registry_poisoned)?.len())
    }
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

fn registry_poisoned(error: impl std::fmt::Display) -> VmiError {
    VmiError::Backend(format!("provider registry synchronization failed: {error}"))
}

fn try_clone_string(value: &str, field: &str) -> Result<String> {
    let mut cloned = String::new();
    cloned.try_reserve_exact(value.len()).map_err(|error| {
        VmiError::Backend(format!("failed to allocate provider {field}: {error}"))
    })?;
    cloned.push_str(value);
    Ok(cloned)
}

fn try_clone_descriptor(descriptor: &ProviderDescriptor) -> Result<ProviderDescriptor> {
    Ok(ProviderDescriptor {
        id: try_clone_string(&descriptor.id, "descriptor ID")?,
        display_name: try_clone_string(&descriptor.display_name, "descriptor display name")?,
        version: descriptor
            .version
            .as_deref()
            .map(|version| try_clone_string(version, "descriptor version"))
            .transpose()?,
        maturity: descriptor.maturity.clone(),
        capabilities: descriptor.capabilities,
    })
}

pub struct VmiSession {
    session: Box<dyn Session>,
    translations: Mutex<TranslationCache>,
}

impl VmiSession {
    pub fn attach(connector: &dyn Connector, request: AttachRequest) -> Result<Self> {
        Ok(Self {
            session: connector.connect(request)?,
            translations: Mutex::new(TranslationCache::new(4096)?),
        })
    }

    pub fn session(&self) -> &dyn Session {
        self.session.as_ref()
    }

    pub fn read_bytes(&self, address: Gpa, length: usize) -> Result<Vec<u8>> {
        read_bytes(self.session.memory()?, address, length)
    }

    pub fn read_scalar<T: Scalar>(&self, address: Gpa, order: ByteOrder) -> Result<T> {
        read_scalar(self.session.memory()?, address, order)
    }

    pub fn write_bytes(&self, address: Gpa, data: &[u8]) -> Result<()> {
        let result = write_bytes(self.session.memory_write()?, address, data);
        let invalidation = self.clear_translation_cache();
        result?;
        invalidation
    }

    pub fn translate(
        &self,
        translator: &dyn AddressTranslator,
        root: TranslationRoot,
        address: Gva,
    ) -> Result<Translation> {
        let page_address = address.raw() & !0xfff;
        let page_offset = address
            .raw()
            .checked_sub(page_address)
            .ok_or_else(|| VmiError::Backend("virtual page offset underflow".into()))?;
        let key = (translator.cache_tag(), root.raw(), page_address);
        if let Some(value) = self
            .translations
            .lock()
            .map_err(translation_cache_poisoned)?
            .get(key)
        {
            return offset_translation(value, page_offset, address);
        }
        let value = translator.translate(self.session.memory()?, root, Gva::new(page_address))?;
        self.translations
            .lock()
            .map_err(translation_cache_poisoned)?
            .insert(key, value)?;
        offset_translation(value, page_offset, address)
    }

    pub fn read_virtual(
        &self,
        translator: &dyn AddressTranslator,
        root: TranslationRoot,
        address: Gva,
        length: usize,
    ) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        output.try_reserve_exact(length).map_err(|error| {
            vmi_types::VmiError::Backend(format!(
                "failed to allocate {length}-byte virtual read buffer: {error}"
            ))
        })?;
        output.resize(length, 0);
        let memory = self.session.memory()?;
        let mut completed = 0usize;
        while completed < length {
            let completed_address = u64::try_from(completed).map_err(|_| VmiError::ReadFailed {
                address: address.raw(),
                length,
            })?;
            let current = address.raw().checked_add(completed_address).ok_or(
                vmi_types::VmiError::ReadFailed {
                    address: address.raw(),
                    length,
                },
            )?;
            let page_remaining = page_remaining(current)?;
            let remaining = length.checked_sub(completed).ok_or(VmiError::ReadFailed {
                address: address.raw(),
                length,
            })?;
            let chunk_length = page_remaining.min(remaining);
            let chunk_end = completed.checked_add(chunk_length).ok_or_else(|| {
                vmi_types::VmiError::Backend("virtual read chunk overflow".into())
            })?;
            let chunk = output.get_mut(completed..chunk_end).ok_or_else(|| {
                vmi_types::VmiError::Backend("virtual read chunk is out of bounds".into())
            })?;
            let translation = self.translate(translator, root, Gva::new(current))?;
            memory.read_into(translation.physical_address, chunk)?;
            completed = chunk_end;
        }
        Ok(output)
    }

    pub fn write_virtual(
        &self,
        translator: &dyn AddressTranslator,
        root: TranslationRoot,
        address: Gva,
        data: &[u8],
    ) -> Result<()> {
        let result: Result<()> = (|| {
            let memory = self.session.memory_write()?;
            let mut completed = 0usize;
            while completed < data.len() {
                let completed_address =
                    u64::try_from(completed).map_err(|_| VmiError::ReadFailed {
                        address: address.raw(),
                        length: data.len(),
                    })?;
                let current = address.raw().checked_add(completed_address).ok_or(
                    vmi_types::VmiError::ReadFailed {
                        address: address.raw(),
                        length: data.len(),
                    },
                )?;
                let page_remaining = page_remaining(current)?;
                let remaining = data.len().checked_sub(completed).ok_or_else(|| {
                    VmiError::Backend("virtual write progress exceeds input length".into())
                })?;
                let chunk_length = page_remaining.min(remaining);
                let chunk_end = completed.checked_add(chunk_length).ok_or_else(|| {
                    vmi_types::VmiError::Backend("virtual write chunk overflow".into())
                })?;
                let chunk = data.get(completed..chunk_end).ok_or_else(|| {
                    vmi_types::VmiError::Backend("virtual write chunk is out of bounds".into())
                })?;
                let translation = self.translate(translator, root, Gva::new(current))?;
                memory.write(translation.physical_address, chunk)?;
                completed = chunk_end;
            }
            Ok(())
        })();
        let invalidation = self.clear_translation_cache();
        result?;
        invalidation
    }

    pub fn clear_translation_cache(&self) -> Result<()> {
        self.translations
            .lock()
            .map_err(translation_cache_poisoned)?
            .clear();
        Ok(())
    }
}

fn translation_cache_poisoned(error: impl std::fmt::Display) -> VmiError {
    VmiError::Backend(format!("translation cache synchronization failed: {error}"))
}

fn offset_translation(
    translation: Translation,
    page_offset: u64,
    address: Gva,
) -> Result<Translation> {
    let physical = translation
        .physical_address
        .raw()
        .checked_add(page_offset)
        .ok_or(VmiError::ReadFailed {
            address: address.raw(),
            length: 1,
        })?;
    Ok(Translation::new(Gpa::new(physical), translation.page_size))
}

struct TranslationCache {
    capacity: usize,
    entries: HashMap<(u64, u64, u64), Translation>,
    order: VecDeque<(u64, u64, u64)>,
}

impl TranslationCache {
    fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(VmiError::Backend(
                "translation cache capacity must be non-zero".into(),
            ));
        }
        let allocation = capacity
            .checked_add(1)
            .ok_or_else(|| VmiError::Backend("translation cache capacity overflow".into()))?;
        let mut entries = HashMap::new();
        entries.try_reserve(allocation).map_err(|error| {
            VmiError::Backend(format!("failed to allocate translation cache: {error}"))
        })?;
        let mut order = VecDeque::new();
        order.try_reserve_exact(allocation).map_err(|error| {
            VmiError::Backend(format!(
                "failed to allocate translation cache order: {error}"
            ))
        })?;
        Ok(Self {
            capacity,
            entries,
            order,
        })
    }
    fn get(&self, key: (u64, u64, u64)) -> Option<Translation> {
        self.entries.get(&key).copied()
    }
    fn insert(&mut self, key: (u64, u64, u64), value: Translation) -> Result<()> {
        if self.entries.insert(key, value).is_some() {
            return Ok(());
        }
        self.order.push_back(key);
        if self.entries.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        Ok(())
    }
    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use vmi_testkit::FakeConnector;
    use vmi_types::{Capability, CapabilitySet};

    struct LinearTranslator(AtomicUsize);

    struct ReentrantDescriptorConnector {
        descriptor: ProviderDescriptor,
        registry: std::sync::Weak<ProviderRegistry>,
        registry_write_available: AtomicBool,
    }

    impl Connector for ReentrantDescriptorConnector {
        fn descriptor(&self) -> &ProviderDescriptor {
            let write_available = self
                .registry
                .upgrade()
                .is_some_and(|registry| registry.providers.try_write().is_ok());
            self.registry_write_available
                .store(write_available, Ordering::Relaxed);
            &self.descriptor
        }

        fn connect(&self, _request: AttachRequest) -> Result<Box<dyn Session>> {
            Err(VmiError::Backend("test connector cannot attach".into()))
        }
    }

    #[test]
    fn translation_cache_rejects_invalid_capacities() {
        assert!(TranslationCache::new(0).is_err());
        assert!(TranslationCache::new(usize::MAX).is_err());
    }
    impl AddressTranslator for LinearTranslator {
        fn cache_tag(&self) -> u64 {
            0x1000
        }

        fn translate(
            &self,
            _memory: &dyn vmi_driver_api::MemoryAccess,
            _root: TranslationRoot,
            address: Gva,
        ) -> Result<Translation> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(Translation::new(Gpa::new(address.raw() + 0x1000), 4096))
        }
    }

    struct OverflowTranslator;
    impl AddressTranslator for OverflowTranslator {
        fn cache_tag(&self) -> u64 {
            u64::MAX
        }

        fn translate(
            &self,
            _memory: &dyn vmi_driver_api::MemoryAccess,
            _root: TranslationRoot,
            _address: Gva,
        ) -> Result<Translation> {
            Ok(Translation::new(Gpa::new(u64::MAX), 4096))
        }
    }

    struct OffsetTranslator {
        offset: u64,
        calls: AtomicUsize,
    }
    impl AddressTranslator for OffsetTranslator {
        fn cache_tag(&self) -> u64 {
            self.offset
        }

        fn translate(
            &self,
            _memory: &dyn vmi_driver_api::MemoryAccess,
            _root: TranslationRoot,
            address: Gva,
        ) -> Result<Translation> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(Translation::new(
                Gpa::new(address.raw() + self.offset),
                4096,
            ))
        }
    }

    #[test]
    fn reads_across_virtual_pages_and_reuses_page_cache() {
        let connector =
            FakeConnector::default().with_segment(0x1ffc_u64, (0u8..8).collect::<Vec<_>>());
        let session = VmiSession::attach(
            &connector,
            AttachRequest::any(CapabilitySet::from_caps([Capability::MemoryRead])),
        )
        .unwrap();
        let translator = LinearTranslator(AtomicUsize::new(0));
        assert_eq!(
            session
                .read_virtual(&translator, TranslationRoot::new(0), Gva::new(0xffc), 8)
                .unwrap(),
            (0u8..8).collect::<Vec<_>>()
        );
        session
            .read_virtual(&translator, TranslationRoot::new(0), Gva::new(0xffd), 6)
            .unwrap();
        assert_eq!(translator.0.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn writes_across_virtual_pages_and_invalidates_cache() {
        let capabilities =
            CapabilitySet::from_caps([Capability::MemoryRead, Capability::MemoryWrite]);
        let connector = FakeConnector::default()
            .with_capabilities(capabilities)
            .with_segment(0x1ffc_u64, vec![0u8; 8]);
        let session = VmiSession::attach(&connector, AttachRequest::any(capabilities)).unwrap();
        let translator = LinearTranslator(AtomicUsize::new(0));
        session
            .write_virtual(
                &translator,
                TranslationRoot::new(0),
                Gva::new(0xffc),
                &[1, 2, 3, 4, 5, 6, 7, 8],
            )
            .unwrap();
        assert_eq!(
            session
                .read_virtual(&translator, TranslationRoot::new(0), Gva::new(0xffc), 8)
                .unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        assert_eq!(translator.0.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn partial_virtual_write_failure_still_invalidates_cache() {
        let capabilities =
            CapabilitySet::from_caps([Capability::MemoryRead, Capability::MemoryWrite]);
        let connector = FakeConnector::default()
            .with_capabilities(capabilities)
            .with_segment(0x1ffc_u64, vec![0u8; 4]);
        let session = VmiSession::attach(&connector, AttachRequest::any(capabilities)).unwrap();
        let translator = LinearTranslator(AtomicUsize::new(0));
        assert!(session
            .write_virtual(
                &translator,
                TranslationRoot::new(0),
                Gva::new(0xffc),
                &[1, 2, 3, 4, 5, 6, 7, 8],
            )
            .is_err());
        assert_eq!(translator.0.load(Ordering::Relaxed), 2);
        assert_eq!(
            session
                .read_virtual(&translator, TranslationRoot::new(0), Gva::new(0xffc), 4)
                .unwrap(),
            [1, 2, 3, 4]
        );
        assert_eq!(translator.0.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn registry_lists_attaches_and_rejects_duplicate_providers() {
        let registry = ProviderRegistry::new();
        let connector: Arc<dyn Connector> =
            Arc::new(FakeConnector::default().with_segment(0x1000_u64, vec![9, 8, 7]));
        registry.register(Arc::clone(&connector)).unwrap();
        assert_eq!(registry.len().unwrap(), 1);
        assert_eq!(registry.descriptors().unwrap()[0].id, "fake-read-only");
        assert!(registry.register(connector).is_err());
        let session = registry
            .attach("fake-read-only", AttachRequest::default())
            .unwrap();
        assert_eq!(session.read_bytes(Gpa::new(0x1000), 3).unwrap(), [9, 8, 7]);
        registry.unregister("fake-read-only").unwrap();
        assert!(registry.is_empty().unwrap());
        assert!(registry
            .attach("missing", AttachRequest::default())
            .is_err());
    }

    #[test]
    fn registry_releases_lock_before_descriptor_callbacks() {
        let registry = Arc::new(ProviderRegistry::new());
        let mut descriptor = FakeConnector::default().descriptor().clone();
        descriptor.id = "reentrant-descriptor".into();
        descriptor.version = Some("1.2.3".into());
        let connector = Arc::new(ReentrantDescriptorConnector {
            descriptor,
            registry: Arc::downgrade(&registry),
            registry_write_available: AtomicBool::new(false),
        });
        let erased: Arc<dyn Connector> = connector.clone();
        registry.register(erased).unwrap();

        connector
            .registry_write_available
            .store(false, Ordering::Relaxed);
        let descriptors = registry.descriptors().unwrap();
        assert_eq!(
            descriptors.as_slice(),
            std::slice::from_ref(&connector.descriptor)
        );
        assert!(connector.registry_write_available.load(Ordering::Relaxed));
    }

    #[test]
    fn poisoned_translation_cache_fails_closed() {
        let connector = FakeConnector::default().with_segment(0x1000_u64, vec![0; 4096]);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = session.translations.lock().unwrap();
            panic!("poison translation cache for test");
        }));
        assert!(session.clear_translation_cache().is_err());
        assert!(session
            .translate(
                &LinearTranslator(AtomicUsize::new(0)),
                TranslationRoot::new(0),
                Gva::new(0),
            )
            .is_err());
    }

    #[test]
    fn translation_offset_overflow_fails_closed() {
        let connector = FakeConnector::default().with_segment(0x1000_u64, vec![0; 4096]);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        assert!(session
            .translate(&OverflowTranslator, TranslationRoot::new(0), Gva::new(1),)
            .is_err());
    }

    #[test]
    fn translation_cache_separates_translator_instances() {
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, vec![1; 4096])
            .with_segment(0x2000_u64, vec![2; 4096]);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let first = OffsetTranslator {
            offset: 0x1000,
            calls: AtomicUsize::new(0),
        };
        let second = OffsetTranslator {
            offset: 0x2000,
            calls: AtomicUsize::new(0),
        };
        let root = TranslationRoot::new(0);
        assert_eq!(
            session.translate(&first, root, Gva::new(0)).unwrap(),
            Translation::new(Gpa::new(0x1000), 4096)
        );
        assert_eq!(
            session.translate(&second, root, Gva::new(0)).unwrap(),
            Translation::new(Gpa::new(0x2000), 4096)
        );
        assert_eq!(first.calls.load(Ordering::Relaxed), 1);
        assert_eq!(second.calls.load(Ordering::Relaxed), 1);
    }
}
