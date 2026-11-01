//! Reading a file the Android picker handed us.
//!
//! The picker returns a `content://` URI, not a path: the app is granted access
//! to a descriptor opened on its behalf by the provider, and there is no
//! filesystem location it can open later. So the moment a share is created we
//! ask the ContentResolver for the descriptor and keep it for the life of the
//! share — no copy into app storage, which is the whole point of Tunneldrop
//! (a 4 GB video should not have to be duplicated to be shared).
//!
//! All of this is JNI against the Activity context, which `android_context()`
//! below fetches from tao.

use anyhow::{anyhow, Context, Result};
use jni::objects::{JObject, JString, JValue};
use jni::JNIEnv;
use std::ffi::c_void;
use std::os::fd::{FromRawFd, OwnedFd};

pub struct PickedFile {
    pub fd: OwnedFd,
    pub name: String,
    pub size: u64,
}

/// Turns a pending Java exception into an error, so a provider that refuses
/// (revoked grant, deleted file) surfaces as a message rather than a crash the
/// next time any JNI call runs.
fn check_exception(env: &mut JNIEnv, what: &str) -> Result<()> {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err(anyhow!("{what} threw a Java exception"));
    }
    Ok(())
}

/// The JavaVM and the Activity object this app is running in.
///
/// Deliberately not `ndk_context::android_context()`. That global is populated
/// by ndk-glue, which Tauri's stack does not use, so the call does not fail
/// politely — it panics, and a panic crossing tao's FFI boundary aborts the
/// process. tao owns the Activity here and registers it in its own map, so ask
/// tao.
///
/// Because the answer comes out of tao's registry, our `tao` dependency has to
/// resolve to the same version `tauri-runtime-wry` uses: two copies of tao in
/// one binary means two registries, and ours would always be empty. Cargo.lock
/// pins that; if this ever starts reporting no activity after a dependency
/// bump, a duplicated tao is the first thing to check.
fn android_context() -> Result<(jni::JavaVM, *mut c_void)> {
    let ctx = tao::platform::android::prelude::main_android_context()
        .ok_or_else(|| anyhow!("no Android activity is registered yet"))?;
    // SAFETY: tao hands out the process-wide JavaVM pointer it got from the
    // JNI entry point; it is valid for as long as the app runs.
    let vm = unsafe { jni::JavaVM::from_raw(ctx.java_vm.cast()) }.context("JavaVM")?;
    Ok((vm, ctx.context_jobject))
}

/// Opens a `content://` URI and returns its descriptor, display name and size.
pub fn open_content_uri(uri: &str) -> Result<PickedFile> {
    let (vm, context_jobject) = android_context()?;
    // SAFETY: tao holds a global ref to the Activity for the life of the app.
    // The JObject wrapper borrows that ref and does not free it on drop.
    let context = unsafe { JObject::from_raw(context_jobject.cast()) };
    let mut env = vm.attach_current_thread().context("attach thread")?;

    let resolver = env
        .call_method(
            &context,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )
        .and_then(|v| v.l())
        .context("getContentResolver")?;

    let uri_string = env.new_string(uri).context("new_string(uri)")?;
    let parsed = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&uri_string)],
        )
        .and_then(|v| v.l())
        .context("Uri.parse")?;

    // "r" — read-only. We never write to the user's file.
    let mode: JObject = env.new_string("r").context("new_string(mode)")?.into();
    let pfd = env
        .call_method(
            &resolver,
            "openFileDescriptor",
            "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
            &[JValue::Object(&parsed), JValue::Object(&mode)],
        )
        .and_then(|v| v.l())
        .context("openFileDescriptor")?;
    check_exception(&mut env, "openFileDescriptor")?;
    if pfd.is_null() {
        return Err(anyhow!("the provider returned no descriptor for {uri}"));
    }

    let size = env
        .call_method(&pfd, "getStatSize", "()J", &[])
        .and_then(|v| v.j())
        .context("getStatSize")?;
    // -1 means the provider does not know the length (a stream rather than a
    // file). We need it for Content-Length and for the landing page.
    let size = u64::try_from(size)
        .map_err(|_| anyhow!("the provider did not report a size for this file"))?;

    // detachFd hands ownership of the descriptor to us: the ParcelFileDescriptor
    // will not close it when Java collects it, and dropping the OwnedFd will.
    let raw = env
        .call_method(&pfd, "detachFd", "()I", &[])
        .and_then(|v| v.i())
        .context("detachFd")?;
    if raw < 0 {
        return Err(anyhow!("detachFd returned {raw}"));
    }
    // SAFETY: detachFd yielded ownership of a valid descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    let name = display_name(&mut env, &resolver, &parsed).unwrap_or_else(|| "file".to_string());

    Ok(PickedFile { fd, name, size })
}

/// Asks the provider for the file's display name. Best-effort: a provider is
/// free to return nothing, and the name is cosmetic — it only decides what the
/// recipient's browser calls the download.
fn display_name(env: &mut JNIEnv, resolver: &JObject, uri: &JObject) -> Option<String> {
    let null = JObject::null();
    let cursor = env
        .call_method(
            resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;\
             Ljava/lang/String;)Landroid/database/Cursor;",
            &[
                JValue::Object(uri),
                JValue::Object(&null),
                JValue::Object(&null),
                JValue::Object(&null),
                JValue::Object(&null),
            ],
        )
        .ok()?
        .l()
        .ok()?;
    check_exception(env, "query").ok()?;
    if cursor.is_null() {
        return None;
    }

    let name = (|| {
        let moved = env
            .call_method(&cursor, "moveToFirst", "()Z", &[])
            .ok()?
            .z()
            .ok()?;
        if !moved {
            return None;
        }
        // OpenableColumns.DISPLAY_NAME
        let column = env.new_string("_display_name").ok()?;
        let index = env
            .call_method(
                &cursor,
                "getColumnIndex",
                "(Ljava/lang/String;)I",
                &[JValue::Object(&column)],
            )
            .ok()?
            .i()
            .ok()?;
        if index < 0 {
            return None;
        }
        let value = env
            .call_method(
                &cursor,
                "getString",
                "(I)Ljava/lang/String;",
                &[JValue::Int(index)],
            )
            .ok()?
            .l()
            .ok()?;
        if value.is_null() {
            return None;
        }
        let value: String = env.get_string(&JString::from(value)).ok()?.into();
        Some(value)
    })();

    let _ = env.call_method(&cursor, "close", "()V", &[]);
    let _ = env.exception_clear();
    name
}
