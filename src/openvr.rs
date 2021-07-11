use std::{
    ffi::{c_void, CStr, CString},
    mem::MaybeUninit,
};

use anyhow::{anyhow, bail, Context, Result};
use bindings::openvr::{
    k_unMaxApplicationKeyLength, EVRApplicationError, EVRApplicationProperty, EVRApplicationType,
    EVRInitError, IVRApplications_Version, IVRSystem_Version, VREvent_t,
    VR_IVRApplications_FnTable, VR_IVRSystem_FnTable,
};
use cstr::cstr;
use libloading::Library;

pub struct OpenVr {
    library: Library,
}

fn fntable(version: &'static [u8]) -> CString {
    const PREFIX: &[u8; 8] = b"FnTable:";

    assert!(version.len() > 1);
    assert!(!version[0..version.len() - 2].contains(&0));
    assert!(version[version.len() - 1] == 0);

    let mut result = Vec::with_capacity(PREFIX.len() + version.len());
    result.extend_from_slice(PREFIX);
    result.extend_from_slice(version);
    result.pop();

    unsafe { CString::from_vec_unchecked(result) }
}

impl OpenVr {
    pub fn new(application_type: EVRApplicationType) -> Result<Self> {
        unsafe {
            let library = Library::new("openvr_api").context("Failed to load openvr_api.")?;

            let mut error = MaybeUninit::uninit();

            let init2 = library.get::<unsafe extern "C" fn(
                *mut EVRInitError,
                EVRApplicationType,
                *const i8,
            ) -> *mut c_void>(
                cstr!("VR_InitInternal2").to_bytes_with_nul()
            );

            match init2 {
                Ok(init2) => init2(error.as_mut_ptr(), application_type, cstr!("").as_ptr()),
                Err(load_error2) => {
                    let init = library.get::<unsafe extern "C" fn(
                        *mut EVRInitError,
                        EVRApplicationType,
                    ) -> *mut c_void>(
                        cstr!("VR_InitInternal").to_bytes_with_nul()
                    );
                    match init {
                        Ok(init) => init(error.as_mut_ptr(), application_type),
                        Err(load_error) => {
                            bail!("Neither VR_InitInternal2 nor VR_InitInternal were found.\n{:?}\n{:?}", load_error2, load_error)
                        }
                    }
                }
            };

            let error = error.assume_init();
            if error != 0 {
                bail!(
                    "VR init error: {:?}",
                    OpenVr::describe_init_error(&library, error)
                );
            }

            Ok(Self { library })
        }
    }

    fn describe_init_error(library: &Library, error: EVRInitError) -> anyhow::Error {
        unsafe {
            match library.get::<unsafe extern "C" fn(EVRInitError) -> *const i8>(
                cstr!("VR_GetVRInitErrorAsEnglishDescription").to_bytes_with_nul(),
            ) {
                Ok(describe) => anyhow!(
                    "{}: {}",
                    error,
                    CStr::from_ptr(describe(error)).to_string_lossy()
                ),
                Err(_) => anyhow!("{}", error),
            }
        }
    }

    unsafe fn get_generic_interface<'a, T>(&'a self, name: &CStr) -> Result<&'a T> {
        let get = self
            .library
            .get::<unsafe extern "C" fn(*const i8, *mut EVRInitError) -> *const T>(
                b"VR_GetGenericInterface",
            )
            .context("VR_GenericInterface not found")?;
        let mut error = MaybeUninit::uninit();
        let table = get(name.as_ptr(), error.as_mut_ptr());

        let error = error.assume_init();
        if error != 0 {
            bail!(
                "VR get generic interface error: {:?}",
                OpenVr::describe_init_error(&self.library, error)
            );
        }

        Ok(table.as_ref().unwrap())
    }

    pub fn applications(&self) -> Result<VrApplications> {
        unsafe {
            let table = self
                .get_generic_interface(&fntable(IVRApplications_Version))
                .context("Failed to get applications interface")?;

            Ok(VrApplications(table))
        }
    }

    pub fn system(&self) -> Result<VrSystem> {
        unsafe {
            let table = self
                .get_generic_interface(&fntable(IVRSystem_Version))
                .context("Failed to get system interface")?;

            Ok(VrSystem(table))
        }
    }
}

impl Drop for OpenVr {
    fn drop(&mut self) {
        unsafe {
            self.library
                .get::<unsafe extern "C" fn()>(cstr!("VR_ShutdownInternal").to_bytes_with_nul())
                .unwrap()()
        }
    }
}

pub struct VrApplications<'a>(&'a VR_IVRApplications_FnTable);

impl<'a> VrApplications<'a> {
    pub fn add_application_manifest(
        &self,
        path: &CStr,
        temporary: bool,
    ) -> Result<(), EVRApplicationError> {
        unsafe {
            match (self.0.AddApplicationManifest.unwrap())(path.as_ptr() as _, temporary) {
                0 => Ok(()),
                error => Err(error),
            }
        }
    }

    pub fn get_applications_err_name_from_enum(&self, error: EVRApplicationError) -> &'a CStr {
        unsafe { CStr::from_ptr((self.0.GetApplicationsErrorNameFromEnum.unwrap())(error)) }
    }

    pub fn get_application_auto_launch(&self, id: &CStr) -> bool {
        unsafe { (self.0.GetApplicationAutoLaunch.unwrap())(id.as_ptr() as _) }
    }

    pub fn set_application_auto_launch(
        &self,
        id: &CStr,
        autolaunch: bool,
    ) -> Result<(), EVRApplicationError> {
        unsafe {
            match (self.0.SetApplicationAutoLaunch.unwrap())(id.as_ptr() as _, autolaunch) {
                0 => Ok(()),
                error => Err(error),
            }
        }
    }

    pub fn get_current_scene_process_id(&self) -> u32 {
        unsafe { (self.0.GetCurrentSceneProcessId.unwrap())() }
    }

    pub fn get_application_key_by_process_id(&self, process_id: u32) -> Result<CString> {
        unsafe {
            let mut app_key_buffer: [MaybeUninit<_>; k_unMaxApplicationKeyLength as usize] =
                MaybeUninit::uninit().assume_init();
            match (self.0.GetApplicationKeyByProcessId.unwrap())(
                process_id,
                app_key_buffer[0].as_mut_ptr(),
                k_unMaxApplicationKeyLength,
            ) {
                0 => {
                    let mut len = 0;
                    loop {
                        if len < k_unMaxApplicationKeyLength as usize
                            && app_key_buffer[len].assume_init() == 0
                        {
                            break;
                        }
                        len += 1;
                    }
                    let initialized: &[MaybeUninit<i8>] = &app_key_buffer[0..len];
                    let initialized: &[u8] = &*(initialized as *const _ as *const _);
                    let mut vec = Vec::with_capacity(len + 1);
                    vec.extend_from_slice(initialized);
                    Ok(CString::from_vec_unchecked(vec))
                }
                error => bail!(
                    "GetApplicationKeyByProcessId error {}: {}",
                    error,
                    self.get_applications_err_name_from_enum(error)
                        .to_string_lossy()
                ),
            }
        }
    }

    pub fn get_application_property_string(
        &self,
        app_key: &CStr,
        property: EVRApplicationProperty,
    ) -> Result<String> {
        unsafe {
            let mut result = Vec::new();
            loop {
                let mut error = MaybeUninit::uninit();
                let len = result.capacity() as u32;
                let needed = (self.0.GetApplicationPropertyString.unwrap())(
                    app_key.as_ptr() as _,
                    property,
                    result.as_mut_ptr() as _,
                    len,
                    error.as_mut_ptr(),
                );
                let error = error.assume_init();
                if error != 0 {
                    bail!(
                        "GetApplicationPropertyString error {}: {}",
                        error,
                        self.get_applications_err_name_from_enum(error)
                            .to_string_lossy()
                    );
                }
                if needed > len {
                    result.reserve_exact(needed as usize);
                } else {
                    // Ignore null terminator.
                    result.set_len(needed as usize - 1);
                    return String::from_utf8(result).context("Invalid characters in string");
                }
            }
        }
    }
}

pub struct VrSystem<'a>(&'a VR_IVRSystem_FnTable);

impl<'a> VrSystem<'a> {
    pub fn poll_next_event(&self) -> Option<VREvent_t> {
        unsafe {
            let mut event = MaybeUninit::uninit();
            if (self.0.PollNextEvent.unwrap())(
                event.as_mut_ptr(),
                std::mem::size_of_val(&event) as _,
            ) {
                Some(event.assume_init())
            } else {
                None
            }
        }
    }

    pub fn acknowledge_quit_exiting(&self) {
        unsafe { (self.0.AcknowledgeQuit_Exiting.unwrap())() }
    }
}
