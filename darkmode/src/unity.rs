//! Thin wrappers around the Unity / IL2CPP functions the plugin calls.
//!
//! Managed (C#) methods compiled by IL2CPP take a trailing `MethodInfo*`, which we pass as null.
//! Internal calls (icalls) don't.

use std::ffi::{c_void, CStr};
use std::ptr::null_mut;
use std::sync::OnceLock;

use crate::api::{self, api, log_warn, Ptr};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

const NULL_MI: Ptr = null_mut();
const ARRAY_LENGTH_OFFSET: usize = 0x18;
const ARRAY_DATA_OFFSET: usize = 0x20;

pub struct Unity {
    pub texture2d_class: usize,
    pub atlas_reference_class: usize,
    byte_class: usize,
    atlas_sprites_field: usize,

    object_get_name: usize,
    object_set_name: usize,
    object_destroy: usize,
    texture_get_data_width: usize,
    texture_get_data_height: usize,
    texture2d_ctor: usize,
    texture2d_read_pixels: usize,
    render_texture_get_temporary: usize,
    render_texture_release_temporary: usize,
    render_texture_get_active: usize,
    render_texture_set_active: usize,
    graphics_blit: usize,
    image_conversion_load_image: usize,
    image_conversion_encode_to_png: usize,
    sprite_get_texture: usize,
    resources_find_objects_of_type_all: usize,
    texture2d_type_object: usize,
    pub image_class: usize,
    pub raw_image_class: usize,
    image_get_override_sprite: usize,
    raw_image_get_texture: usize,
    sprite_get_texture_rect: usize,
    sprite_get_packed: usize,
    pub sprite_class: usize,
    graphic_get_color: usize,
    graphic_set_color: usize,
    pub text_common_class: usize,
    pub text_common_update_color: usize,
    pub text_common_rebuild_outline: usize,
    text_common_force_update_outline: usize,
    pub ui_manager_late_update: usize,
    pub graphic_do_mesh_generation: usize,
    pub camera_class: usize,
    pub renderer_class: usize,
    renderer_get_shared_material: usize,
    renderer_get_enabled: usize,
    material_get_color: usize,
    material_get_main_texture: usize,
    component_get_game_object: usize,
    game_object_active_in_hierarchy: usize,
    pub outline_common_apply_outline_color: usize,
    pub outline_common_set_color: usize,
    pub outline_common_class: usize,
    pub shadow_modify_mesh: usize,
    pub outline_modify_mesh: usize,
    base_mesh_effect_get_graphic: usize,
    shadow_get_effect_color: usize,
    shadow_effect_color_field: usize,
    pub text_class: usize,
    pub image_common_class: usize,
    pub gradient_common_modify_mesh: usize,
    gradient_common_get_text_common: usize,
    image_common_get_recieve_color: usize,
    image_common_set_recieve_color: usize,
    pub transform_class: usize,
    graphic_get_canvas_renderer: usize,
    canvas_renderer_get_color: usize,
    canvas_renderer_set_color: usize,
    canvas_renderer_color_injected: bool,
    game_object_set_active: usize,
    outline_common_get_color: usize,
    outline_common_get_text_common: usize,
    camera_get_background_color: usize,
    camera_get_clear_flags: usize,
    camera_set_background_color: usize,
    behaviour_get_is_active_and_enabled: usize,
    texture2d_get_pixels32: usize,
    application_get_version: usize,
    graphic_color_field: usize,
    object_is_native_object_alive: usize,
    pub text_common_set_gradient1: usize,
    pub text_common_set_gradient2: usize,
    pub text_common_set_gradient_vertical: usize,
    text_common_get_gradient1: usize,
    text_common_get_gradient2: usize,
    component_get_transform: usize,
    transform_get_parent: usize,

    // Hook targets
    pub asset_handle_set_object_cache: usize,
    pub atlas_reference_on_enable: usize,
}

static UNITY: OnceLock<Unity> = OnceLock::new();

pub fn unity() -> &'static Unity {
    UNITY.get().expect("darkmode: Unity symbols not initialized")
}

fn image(name: &CStr) -> Ptr {
    let img = unsafe { (api().il2cpp_get_assembly_image)(name.as_ptr()) };
    if img.is_null() {
        log_warn!("Assembly not found: {}", name.to_string_lossy());
    }
    img
}

fn class(image: Ptr, namespace: &CStr, name: &CStr) -> usize {
    if image.is_null() {
        return 0;
    }
    let c = unsafe { (api().il2cpp_get_class)(image, namespace.as_ptr(), name.as_ptr()) } as usize;
    if c == 0 {
        log_warn!("Class not found: {}.{}", namespace.to_string_lossy(), name.to_string_lossy());
    }
    c
}

fn method(class: usize, name: &CStr, args: i32) -> usize {
    if class == 0 {
        return 0;
    }
    let m = unsafe { (api().il2cpp_get_method_addr)(class as Ptr, name.as_ptr(), args) } as usize;
    if m == 0 {
        log_warn!("Method not found: {}({} args)", name.to_string_lossy(), args);
    }
    m
}

fn icall(name: &CStr) -> usize {
    let m = unsafe { (api().il2cpp_resolve_icall)(name.as_ptr()) } as usize;
    if m == 0 {
        log_warn!("icall not found: {}", name.to_string_lossy());
    }
    m
}

/// The `System.Type` object for a class (what C# `typeof(T)` returns).
fn type_object(class: usize) -> usize {
    if class == 0 {
        return 0;
    }
    let get_type = unsafe { (api().il2cpp_resolve_symbol)(c"il2cpp_class_get_type".as_ptr()) };
    let get_object = unsafe { (api().il2cpp_resolve_symbol)(c"il2cpp_type_get_object".as_ptr()) };
    if get_type.is_null() || get_object.is_null() {
        return 0;
    }
    let get_type: extern "C" fn(Ptr) -> Ptr = unsafe { std::mem::transmute(get_type) };
    let get_object: extern "C" fn(Ptr) -> Ptr = unsafe { std::mem::transmute(get_object) };
    get_object(get_type(class as Ptr)) as usize
}

pub fn init() -> bool {
    let core = image(c"UnityEngine.CoreModule.dll");
    let cyan = image(c"_Cyan.dll");
    let imgconv = image(c"UnityEngine.ImageConversionModule.dll");
    let mscorlib = image(c"mscorlib.dll");
    let cute_ui = image(c"Cute.UI.Assembly.dll");

    let object = class(core, c"UnityEngine", c"Object");
    let texture2d = class(core, c"UnityEngine", c"Texture2D");
    let camera = class(core, c"UnityEngine", c"Camera");
    let renderer = class(core, c"UnityEngine", c"Renderer");
    let material = class(core, c"UnityEngine", c"Material");
    let game_object = class(core, c"UnityEngine", c"GameObject");
    let behaviour = class(core, c"UnityEngine", c"Behaviour");
    let render_texture = class(core, c"UnityEngine", c"RenderTexture");
    let sprite = class(core, c"UnityEngine", c"Sprite");
    let resources = class(core, c"UnityEngine", c"Resources");
    let ui = image(c"UnityEngine.UI.dll");
    let ui_image = class(ui, c"UnityEngine.UI", c"Image");
    let ui_raw_image = class(ui, c"UnityEngine.UI", c"RawImage");
    let graphic = class(ui, c"UnityEngine.UI", c"Graphic");
    let ui_shadow = class(ui, c"UnityEngine.UI", c"Shadow");
    let ui_outline = class(ui, c"UnityEngine.UI", c"Outline");
    let ui_base_mesh_effect = class(ui, c"UnityEngine.UI", c"BaseMeshEffect");
    let ui_text = class(ui, c"UnityEngine.UI", c"Text");
    // CanvasRenderer colour icalls: plain in some Unity versions, `_Injected` (struct by pointer)
    // in others.
    let quiet_icall = |name: &CStr| unsafe { (api().il2cpp_resolve_icall)(name.as_ptr()) as usize };
    let (cr_get, cr_set, cr_injected) = {
        let get = quiet_icall(c"UnityEngine.CanvasRenderer::GetColor_Injected(UnityEngine.Color&)");
        let set = quiet_icall(c"UnityEngine.CanvasRenderer::SetColor_Injected(UnityEngine.Color&)");
        if get != 0 && set != 0 {
            (get, set, true)
        } else {
            (icall(c"UnityEngine.CanvasRenderer::GetColor()"), icall(c"UnityEngine.CanvasRenderer::SetColor(UnityEngine.Color)"), false)
        }
    };
    let component = class(core, c"UnityEngine", c"Component");
    let transform = class(core, c"UnityEngine", c"Transform");
    let umamusume = image(c"umamusume.dll");
    let text_common = class(umamusume, c"Gallop", c"TextCommon");
    let ui_manager = class(umamusume, c"Gallop", c"UIManager");
    let outline_common = class(umamusume, c"Gallop", c"OutlineCommon");
    let image_common = class(umamusume, c"Gallop", c"ImageCommon");
    let gradient_common = class(umamusume, c"Gallop", c"GradientCommon");
    let image_conversion = class(imgconv, c"UnityEngine", c"ImageConversion");
    let asset_handle = class(cyan, c"Cyan.Loader", c"AssetHandle");
    let byte = class(mscorlib, c"System", c"Byte");
    let atlas_reference = class(cute_ui, c"Cute.UI", c"AtlasReference");

    let atlas_sprites_field = if atlas_reference != 0 {
        unsafe { (api().il2cpp_get_field_from_name)(atlas_reference as Ptr, c"sprites".as_ptr()) as usize }
    } else {
        0
    };

    let u = Unity {
        texture2d_class: texture2d,
        atlas_reference_class: atlas_reference,
        byte_class: byte,
        atlas_sprites_field,

        object_get_name: method(object, c"get_name", 0),
        object_set_name: method(object, c"set_name", 1),
        object_destroy: method(object, c"Destroy", 1),
        texture_get_data_width: icall(c"UnityEngine.Texture::GetDataWidth()"),
        texture_get_data_height: icall(c"UnityEngine.Texture::GetDataHeight()"),
        texture2d_ctor: method(texture2d, c".ctor", 2),
        texture2d_read_pixels: method(texture2d, c"ReadPixels", 3),
        render_texture_get_temporary: method(render_texture, c"GetTemporary", 2),
        render_texture_release_temporary: icall(c"UnityEngine.RenderTexture::ReleaseTemporary(UnityEngine.RenderTexture)"),
        render_texture_get_active: icall(c"UnityEngine.RenderTexture::GetActive()"),
        render_texture_set_active: icall(c"UnityEngine.RenderTexture::SetActive(UnityEngine.RenderTexture)"),
        graphics_blit: icall(c"UnityEngine.Graphics::Blit2(UnityEngine.Texture,UnityEngine.RenderTexture)"),
        image_conversion_load_image: method(image_conversion, c"LoadImage", 3),
        image_conversion_encode_to_png: method(image_conversion, c"EncodeToPNG", 1),
        sprite_get_texture: method(sprite, c"get_texture", 0),
        resources_find_objects_of_type_all: method(resources, c"FindObjectsOfTypeAll", 1),
        texture2d_type_object: type_object(texture2d),
        image_class: ui_image,
        raw_image_class: ui_raw_image,
        image_get_override_sprite: method(ui_image, c"get_overrideSprite", 0),
        raw_image_get_texture: method(ui_raw_image, c"get_texture", 0),
        sprite_get_texture_rect: method(sprite, c"get_textureRect", 0),
        sprite_get_packed: method(sprite, c"get_packed", 0),
        sprite_class: sprite,
        graphic_get_color: method(graphic, c"get_color", 0),
        graphic_set_color: method(graphic, c"set_color", 1),
        text_common_class: text_common,
        text_common_update_color: method(text_common, c"UpdateColor", 0),
        text_common_rebuild_outline: method(text_common, c"RebuildOutline", 1),
        text_common_force_update_outline: method(text_common, c"ForceUpdateOutLine", 0),
        ui_manager_late_update: method(ui_manager, c"LateUpdate", 0),
        graphic_do_mesh_generation: method(graphic, c"DoMeshGeneration", 0),
        camera_class: camera,
        renderer_class: renderer,
        renderer_get_shared_material: method(renderer, c"get_sharedMaterial", 0),
        renderer_get_enabled: method(renderer, c"get_enabled", 0),
        material_get_color: method(material, c"get_color", 0),
        material_get_main_texture: method(material, c"get_mainTexture", 0),
        component_get_game_object: method(component, c"get_gameObject", 0),
        game_object_active_in_hierarchy: method(game_object, c"get_activeInHierarchy", 0),
        outline_common_apply_outline_color: method(outline_common, c"ApplyOutlineColor", 1),
        outline_common_set_color: method(outline_common, c"set_color", 1),
        outline_common_class: outline_common,
        shadow_modify_mesh: method(ui_shadow, c"ModifyMesh", 1),
        outline_modify_mesh: method(ui_outline, c"ModifyMesh", 1),
        base_mesh_effect_get_graphic: method(ui_base_mesh_effect, c"get_graphic", 0),
        shadow_get_effect_color: method(ui_shadow, c"get_effectColor", 0),
        shadow_effect_color_field: if ui_shadow != 0 { unsafe { (api().il2cpp_get_field_from_name)(ui_shadow as Ptr, c"m_EffectColor".as_ptr()) as usize } } else { 0 },
        text_class: ui_text,
        image_common_class: image_common,
        gradient_common_modify_mesh: method(gradient_common, c"ModifyMesh", 1),
        gradient_common_get_text_common: method(gradient_common, c"get_TextCommon", 0),
        image_common_get_recieve_color: method(image_common, c"get_recieveColor", 0),
        image_common_set_recieve_color: method(image_common, c"set_recieveColor", 1),
        transform_class: transform,
        graphic_get_canvas_renderer: method(graphic, c"get_canvasRenderer", 0),
        canvas_renderer_get_color: cr_get,
        canvas_renderer_set_color: cr_set,
        canvas_renderer_color_injected: cr_injected,
        game_object_set_active: method(game_object, c"SetActive", 1),
        outline_common_get_color: method(outline_common, c"get_color", 0),
        outline_common_get_text_common: method(outline_common, c"get_TextCommon", 0),
        camera_get_background_color: method(camera, c"get_backgroundColor", 0),
        camera_get_clear_flags: method(camera, c"get_clearFlags", 0),
        camera_set_background_color: method(camera, c"set_backgroundColor", 1),
        behaviour_get_is_active_and_enabled: method(behaviour, c"get_isActiveAndEnabled", 0),
        texture2d_get_pixels32: icall(c"UnityEngine.Texture2D::GetPixels32(System.Int32)"),
        application_get_version: icall(c"UnityEngine.Application::get_version()"),
        graphic_color_field: if graphic != 0 { unsafe { (api().il2cpp_get_field_from_name)(graphic as Ptr, c"m_Color".as_ptr()) as usize } } else { 0 },
        object_is_native_object_alive: method(object, c"IsNativeObjectAlive", 1),
        text_common_set_gradient1: method(text_common, c"set_GradientColor1", 1),
        text_common_set_gradient2: method(text_common, c"set_GradientColor2", 1),
        text_common_set_gradient_vertical: method(text_common, c"SetGradientVertical", 2),
        text_common_get_gradient1: method(text_common, c"get_GradientColor1", 0),
        text_common_get_gradient2: method(text_common, c"get_GradientColor2", 0),
        component_get_transform: method(component, c"get_transform", 0),
        transform_get_parent: method(transform, c"get_parent", 0),

        asset_handle_set_object_cache: method(asset_handle, c"SetObjectCache", 3),
        atlas_reference_on_enable: method(atlas_reference, c"OnEnable", 0),
    };

    let ok = [
        u.texture2d_class, u.byte_class, u.object_get_name, u.object_set_name,
        u.texture_get_data_width, u.texture_get_data_height, u.texture2d_ctor,
        u.texture2d_read_pixels, u.render_texture_get_temporary,
        u.render_texture_release_temporary, u.render_texture_get_active,
        u.render_texture_set_active, u.graphics_blit, u.image_conversion_load_image,
        u.image_conversion_encode_to_png,
    ].iter().all(|&a| a != 0);

    let _ = UNITY.set(u);
    ok
}

macro_rules! call {
    ($addr:expr, fn($($t:ty),*) -> $ret:ty, $($arg:expr),*) => {{
        let f: extern "C-unwind" fn($($t),*) -> $ret = unsafe { std::mem::transmute($addr) };
        f($($arg),*)
    }};
}

// ---- object helpers ----

pub fn class_of(obj: Ptr) -> usize {
    if obj.is_null() {
        return 0;
    }
    unsafe { *(obj as *const usize) }
}

pub fn string_to_rust(s: Ptr) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe {
        let chars = (api().il2cpp_string_chars)(s);
        let len = (api().il2cpp_string_length)(s);
        if chars.is_null() || len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(chars, len as usize))
    }
}

pub fn string_new(s: &str) -> Ptr {
    let c = api::cstring(s);
    unsafe { (api().il2cpp_string_new)(c.as_ptr()) }
}

pub fn object_name(obj: Ptr) -> String {
    string_to_rust(call!(unity().object_get_name, fn(Ptr, Ptr) -> Ptr, obj, NULL_MI))
}

pub fn set_object_name(obj: Ptr, name: &str) {
    call!(unity().object_set_name, fn(Ptr, Ptr, Ptr) -> (), obj, string_new(name), NULL_MI)
}

pub fn destroy(obj: Ptr) {
    let u = unity();
    if u.object_destroy != 0 {
        call!(u.object_destroy, fn(Ptr, Ptr) -> (), obj, NULL_MI)
    }
}

// ---- arrays ----

pub fn byte_array_new(data: &[u8]) -> Ptr {
    let arr = unsafe { (api().il2cpp_create_array)(unity().byte_class as Ptr, data.len()) };
    if !arr.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), (arr as *mut u8).add(ARRAY_DATA_OFFSET), data.len());
        }
    }
    arr
}

pub fn byte_array_to_vec(arr: Ptr) -> Vec<u8> {
    if arr.is_null() {
        return Vec::new();
    }
    unsafe {
        let len = *((arr as *const u8).add(ARRAY_LENGTH_OFFSET) as *const usize);
        std::slice::from_raw_parts((arr as *const u8).add(ARRAY_DATA_OFFSET), len).to_vec()
    }
}

/// Returns the elements of an object array (e.g. `Sprite[]`).
pub fn object_array(arr: Ptr) -> Vec<Ptr> {
    if arr.is_null() {
        return Vec::new();
    }
    unsafe {
        let len = *((arr as *const u8).add(ARRAY_LENGTH_OFFSET) as *const usize);
        std::slice::from_raw_parts((arr as *const u8).add(ARRAY_DATA_OFFSET) as *const Ptr, len).to_vec()
    }
}

// ---- textures ----

pub fn texture2d_new(width: i32, height: i32) -> Ptr {
    let u = unity();
    let obj = unsafe { (api().il2cpp_object_new)(u.texture2d_class as Ptr) };
    call!(u.texture2d_ctor, fn(Ptr, i32, i32, Ptr) -> (), obj, width, height, NULL_MI);
    obj
}

pub fn texture_size(tex: Ptr) -> (i32, i32) {
    let u = unity();
    (
        call!(u.texture_get_data_width, fn(Ptr) -> i32, tex),
        call!(u.texture_get_data_height, fn(Ptr) -> i32, tex),
    )
}

/// Replaces a texture's contents with an encoded image (PNG/JPG). The texture is resized as needed.
pub fn load_image(tex: Ptr, encoded: &[u8], mark_non_readable: bool) -> bool {
    let arr = byte_array_new(encoded);
    if arr.is_null() {
        return false;
    }
    call!(unity().image_conversion_load_image, fn(Ptr, Ptr, bool, Ptr) -> bool, tex, arr, mark_non_readable, NULL_MI)
}

/// Copies any texture (even non-readable / GPU-only ones) into a new readable Texture2D.
pub fn readable_copy(tex: Ptr) -> Option<Ptr> {
    let u = unity();
    let (w, h) = texture_size(tex);
    if w <= 0 || h <= 0 {
        return None;
    }

    let rt = call!(u.render_texture_get_temporary, fn(i32, i32, Ptr) -> Ptr, w, h, NULL_MI);
    if rt.is_null() {
        return None;
    }
    call!(u.graphics_blit, fn(Ptr, Ptr) -> (), tex, rt);

    let prev = call!(u.render_texture_get_active, fn() -> Ptr,);
    call!(u.render_texture_set_active, fn(Ptr) -> (), rt);

    let out = texture2d_new(w, h);
    let rect = Rect { x: 0.0, y: 0.0, width: w as f32, height: h as f32 };
    call!(u.texture2d_read_pixels, fn(Ptr, Rect, i32, i32, Ptr) -> (), out, rect, 0, 0, NULL_MI);

    call!(u.render_texture_set_active, fn(Ptr) -> (), prev);
    call!(u.render_texture_release_temporary, fn(Ptr) -> (), rt);
    Some(out)
}

pub fn encode_to_png(tex: Ptr) -> Vec<u8> {
    byte_array_to_vec(call!(unity().image_conversion_encode_to_png, fn(Ptr, Ptr) -> Ptr, tex, NULL_MI))
}

// ---- sprites / atlases ----

pub fn sprite_texture(sprite: Ptr) -> Ptr {
    let u = unity();
    if u.sprite_get_texture == 0 || sprite.is_null() {
        return null_mut();
    }
    call!(u.sprite_get_texture, fn(Ptr, Ptr) -> Ptr, sprite, NULL_MI)
}

pub fn atlas_sprites(atlas: Ptr) -> Vec<Ptr> {
    let u = unity();
    if u.atlas_sprites_field == 0 {
        return Vec::new();
    }
    let mut arr: Ptr = null_mut();
    unsafe {
        (api().il2cpp_get_field_value)(atlas, u.atlas_sprites_field as Ptr, &mut arr as *mut Ptr as *mut c_void);
    }
    object_array(arr)
}

/// Every loaded object of a class (and subclasses), including inactive ones, skipping objects
/// whose native side is already destroyed (calling into those throws managed exceptions, which
/// abort the process when they unwind through Rust). Main thread only.
pub fn find_all_of_class(class: usize) -> Vec<Ptr> {
    let mut objs = find_all_of_class_unchecked(class);
    objs.retain(|&o| is_alive(o));
    objs
}

/// `find_all_of_class` without the liveness filter: check `is_alive` before touching an object.
fn find_all_of_class_unchecked(class: usize) -> Vec<Ptr> {
    let u = unity();
    let ty = if class == u.texture2d_class { u.texture2d_type_object } else { type_object(class) };
    if u.resources_find_objects_of_type_all == 0 || ty == 0 {
        return Vec::new();
    }
    object_array(call!(u.resources_find_objects_of_type_all, fn(Ptr, Ptr) -> Ptr, ty as Ptr, NULL_MI))
}

pub fn is_alive(obj: Ptr) -> bool {
    let u = unity();
    if obj.is_null() {
        return false;
    }
    if u.object_is_native_object_alive == 0 {
        return true;
    }
    call!(u.object_is_native_object_alive, fn(Ptr, Ptr) -> bool, obj, NULL_MI)
}

pub fn all_texture2ds() -> Vec<Ptr> {
    find_all_of_class(unity().texture2d_class)
}

/// All Texture2Ds, not filtered by `is_alive` (see `find_all_of_class_unchecked`).
pub fn all_texture2ds_unchecked() -> Vec<Ptr> {
    find_all_of_class_unchecked(unity().texture2d_class)
}

pub fn image_sprite(image: Ptr) -> Ptr {
    call!(unity().image_get_override_sprite, fn(Ptr, Ptr) -> Ptr, image, NULL_MI)
}

pub fn raw_image_texture(raw: Ptr) -> Ptr {
    call!(unity().raw_image_get_texture, fn(Ptr, Ptr) -> Ptr, raw, NULL_MI)
}

/// The sprite's rect on its texture. `None` for packed sprites, where Unity's `textureRect` throws
/// for tight packing (and a managed exception unwinding through Rust aborts the process).
pub fn sprite_texture_rect(sprite: Ptr) -> Option<Rect> {
    let u = unity();
    if u.sprite_get_packed == 0 || call!(u.sprite_get_packed, fn(Ptr, Ptr) -> bool, sprite, NULL_MI) {
        return None;
    }
    Some(call!(u.sprite_get_texture_rect, fn(Ptr, Ptr) -> Rect, sprite, NULL_MI))
}

/// "Namespace.Name" of an Il2CppClass (name at +0x10, namespace at +0x18).
pub fn class_name(class: usize) -> String {
    if class == 0 {
        return "null".into();
    }
    unsafe {
        let name = *((class + 0x10) as *const *const std::ffi::c_char);
        let ns = *((class + 0x18) as *const *const std::ffi::c_char);
        let name = if name.is_null() { "?".into() } else { CStr::from_ptr(name).to_string_lossy() };
        let ns = if ns.is_null() { "".into() } else { CStr::from_ptr(ns).to_string_lossy() };
        if ns.is_empty() { name.into_owned() } else { format!("{ns}.{name}") }
    }
}

impl Unity {
    pub fn graphic_set_color_addr(&self) -> usize {
        self.graphic_set_color
    }
}

/// True if an object of class `class` can be assigned to `base` (i.e. it is `base` or derives from it).
pub fn class_is_assignable_from(base: usize, class: usize) -> bool {
    static FN: OnceLock<usize> = OnceLock::new();
    let f = *FN.get_or_init(|| unsafe { (api().il2cpp_resolve_symbol)(c"il2cpp_class_is_assignable_from".as_ptr()) as usize });
    if f == 0 {
        return false;
    }
    call!(f, fn(Ptr, Ptr) -> bool, base as Ptr, class as Ptr)
}

/// True if `obj` is an instance of `base` or a subclass. Results are cached per class.
pub fn is_instance_of(obj: Ptr, base: usize) -> bool {
    thread_local! {
        // UI work is main-thread only; a per-thread cache avoids a lock on every call.
        static CACHE: std::cell::RefCell<std::collections::HashMap<(usize, usize), bool>> = Default::default();
    }
    let class = class_of(obj);
    if class == 0 || base == 0 {
        return false;
    }
    if class == base {
        return true;
    }
    CACHE.with(|c| *c.borrow_mut().entry((base, class)).or_insert_with(|| class_is_assignable_from(base, class)))
}

pub fn graphic_color(graphic: Ptr) -> Color {
    call!(unity().graphic_get_color, fn(Ptr, Ptr) -> Color, graphic, NULL_MI)
}

pub fn set_graphic_color(graphic: Ptr, color: Color) {
    call!(unity().graphic_set_color, fn(Ptr, Color, Ptr) -> (), graphic, color, NULL_MI)
}

/// "Root/.../Parent/Self" names of a component's GameObject, up to `depth` levels.
pub fn hierarchy_path(component: Ptr, depth: usize) -> String {
    let u = unity();
    let mut t = call!(u.component_get_transform, fn(Ptr, Ptr) -> Ptr, component, NULL_MI);
    let mut parts = Vec::new();
    while is_alive(t) && parts.len() < depth {
        parts.push(object_name(t));
        t = call!(u.transform_get_parent, fn(Ptr, Ptr) -> Ptr, t, NULL_MI);
    }
    if !t.is_null() {
        parts.push("…".into());
    }
    parts.reverse();
    parts.join("/")
}

pub fn text_force_update_outline(text: Ptr) {
    let u = unity();
    if u.text_common_force_update_outline != 0 {
        call!(u.text_common_force_update_outline, fn(Ptr, Ptr) -> (), text, NULL_MI)
    }
}

pub fn text_gradient_colors(text: Ptr) -> (Color, Color) {
    let u = unity();
    (
        call!(u.text_common_get_gradient1, fn(Ptr, Ptr) -> Color, text, NULL_MI),
        call!(u.text_common_get_gradient2, fn(Ptr, Ptr) -> Color, text, NULL_MI),
    )
}

pub fn set_text_gradient_colors(text: Ptr, c1: Color, c2: Color) {
    let u = unity();
    call!(u.text_common_set_gradient1, fn(Ptr, Color, Ptr) -> (), text, c1, NULL_MI);
    call!(u.text_common_set_gradient2, fn(Ptr, Color, Ptr) -> (), text, c2, NULL_MI);
}

/// Writes `Graphic.m_Color` directly, without the setter (no dirty flags, no hooks).
pub fn write_graphic_color_field(graphic: Ptr, color: Color) {
    let u = unity();
    if u.graphic_color_field != 0 {
        unsafe { (api().il2cpp_set_field_value)(graphic, u.graphic_color_field as Ptr, &color as *const Color as *const c_void) };
    }
}


/// RGBA8 pixels of a readable texture (mip 0), bottom row first, as Unity returns them.
pub fn get_pixels32(tex: Ptr) -> Vec<u8> {
    let u = unity();
    if u.texture2d_get_pixels32 == 0 {
        return Vec::new();
    }
    // Color32[] is laid out as packed RGBA bytes.
    let arr = call!(u.texture2d_get_pixels32, fn(Ptr, i32) -> Ptr, tex, 0);
    if arr.is_null() {
        return Vec::new();
    }
    unsafe {
        let len = *((arr as *const u8).add(ARRAY_LENGTH_OFFSET) as *const usize);
        std::slice::from_raw_parts((arr as *const u8).add(ARRAY_DATA_OFFSET), len * 4).to_vec()
    }
}

/// `Application.version`, the game's version string.
pub fn application_version() -> Option<String> {
    let u = unity();
    if u.application_get_version == 0 {
        return None;
    }
    let s = call!(u.application_get_version, fn() -> Ptr,);
    let v = string_to_rust(s);
    (!v.is_empty()).then_some(v)
}

/// (background colour, clear flags) of a camera.
pub fn camera_background(cam: Ptr) -> (Color, i32) {
    let u = unity();
    (
        call!(u.camera_get_background_color, fn(Ptr, Ptr) -> Color, cam, NULL_MI),
        call!(u.camera_get_clear_flags, fn(Ptr, Ptr) -> i32, cam, NULL_MI),
    )
}

/// Whether a Behaviour (any component that can be enabled) is enabled and on an active object.
pub fn is_active_and_enabled(behaviour: Ptr) -> bool {
    let u = unity();
    u.behaviour_get_is_active_and_enabled != 0
        && call!(u.behaviour_get_is_active_and_enabled, fn(Ptr, Ptr) -> bool, behaviour, NULL_MI)
}

pub fn set_camera_background(cam: Ptr, color: Color) {
    let u = unity();
    if u.camera_set_background_color != 0 {
        call!(u.camera_set_background_color, fn(Ptr, Color, Ptr) -> (), cam, color, NULL_MI)
    }
}

/// The TextCommon an OutlineCommon belongs to (may be null).
pub fn outline_text(outline: Ptr) -> Ptr {
    let u = unity();
    if u.outline_common_get_text_common == 0 {
        return std::ptr::null_mut();
    }
    call!(u.outline_common_get_text_common, fn(Ptr, Ptr) -> Ptr, outline, NULL_MI)
}

/// Diagnostics for a Renderer: (active, material name, main texture name, material colour).
pub fn renderer_info(r: Ptr) -> (bool, String, String, Color) {
    let u = unity();
    let enabled = call!(u.renderer_get_enabled, fn(Ptr, Ptr) -> bool, r, NULL_MI);
    let go = call!(u.component_get_game_object, fn(Ptr, Ptr) -> Ptr, r, NULL_MI);
    let active = enabled && is_alive(go) && call!(u.game_object_active_in_hierarchy, fn(Ptr, Ptr) -> bool, go, NULL_MI);
    let mat = call!(u.renderer_get_shared_material, fn(Ptr, Ptr) -> Ptr, r, NULL_MI);
    if !is_alive(mat) {
        return (active, "null".into(), String::new(), Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 });
    }
    let tex = call!(u.material_get_main_texture, fn(Ptr, Ptr) -> Ptr, mat, NULL_MI);
    let tex_name = if is_alive(tex) { object_name(tex) } else { String::new() };
    let color = call!(u.material_get_color, fn(Ptr, Ptr) -> Color, mat, NULL_MI);
    (active, object_name(mat), tex_name, color)
}

pub fn outline_color(outline: Ptr) -> Color {
    call!(unity().outline_common_get_color, fn(Ptr, Ptr) -> Color, outline, NULL_MI)
}

/// Calls the (hooked) OutlineCommon.set_color.
pub fn set_outline_color(outline: Ptr, color: Color) {
    call!(unity().outline_common_set_color, fn(Ptr, Color, Ptr) -> (), outline, color, NULL_MI)
}

/// The Graphic a mesh effect (Shadow/Outline) modifies.
pub fn mesh_effect_graphic(effect: Ptr) -> Ptr {
    let u = unity();
    if u.base_mesh_effect_get_graphic == 0 {
        return std::ptr::null_mut();
    }
    call!(u.base_mesh_effect_get_graphic, fn(Ptr, Ptr) -> Ptr, effect, NULL_MI)
}

pub fn shadow_effect_color(effect: Ptr) -> Color {
    call!(unity().shadow_get_effect_color, fn(Ptr, Ptr) -> Color, effect, NULL_MI)
}

/// Writes Shadow/Outline.m_EffectColor directly (no dirty flags; safe during mesh rebuild).
pub fn write_shadow_effect_color(effect: Ptr, color: Color) {
    let u = unity();
    if u.shadow_effect_color_field != 0 {
        unsafe { (api().il2cpp_set_field_value)(effect, u.shadow_effect_color_field as Ptr, &color as *const Color as *const c_void) };
    }
}


/// Gallop.ImageCommon's colour received from a parent ColorSender (used instead of / on top of
/// its own colour).
pub fn image_receive_color(img: Ptr) -> Option<Color> {
    let u = unity();
    if u.image_common_get_recieve_color == 0 || !is_instance_of(img, u.image_common_class) {
        return None;
    }
    Some(call!(u.image_common_get_recieve_color, fn(Ptr, Ptr) -> Color, img, NULL_MI))
}

/// Plain field setter (shared code, so not hooked; safe to call any time).
pub fn set_image_receive_color(img: Ptr, color: Color) {
    let u = unity();
    if u.image_common_set_recieve_color != 0 && is_instance_of(img, u.image_common_class) {
        call!(u.image_common_set_recieve_color, fn(Ptr, Color, Ptr) -> (), img, color, NULL_MI)
    }
}

/// The TextCommon a GradientCommon colours (may be null).
pub fn gradient_text(gradient: Ptr) -> Ptr {
    let u = unity();
    if u.gradient_common_get_text_common == 0 {
        return std::ptr::null_mut();
    }
    call!(u.gradient_common_get_text_common, fn(Ptr, Ptr) -> Ptr, gradient, NULL_MI)
}

/// Deactivates the GameObject a component belongs to.
pub fn deactivate(component: Ptr) {
    let u = unity();
    let go = call!(u.component_get_game_object, fn(Ptr, Ptr) -> Ptr, component, NULL_MI);
    if is_alive(go) && u.game_object_set_active != 0 {
        call!(u.game_object_set_active, fn(Ptr, bool, Ptr) -> (), go, false, NULL_MI);
    }
}

/// The colour a Graphic's CanvasRenderer multiplies everything it draws by.
pub fn canvas_renderer_color(graphic: Ptr) -> Option<Color> {
    let u = unity();
    if u.graphic_get_canvas_renderer == 0 || u.canvas_renderer_get_color == 0 {
        return None;
    }
    let cr = call!(u.graphic_get_canvas_renderer, fn(Ptr, Ptr) -> Ptr, graphic, NULL_MI);
    if !is_alive(cr) {
        return None;
    }
    Some(if u.canvas_renderer_color_injected {
        let mut c = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
        call!(u.canvas_renderer_get_color, fn(Ptr, *mut Color) -> (), cr, &mut c);
        c
    } else {
        call!(u.canvas_renderer_get_color, fn(Ptr) -> Color, cr)
    })
}

pub fn set_canvas_renderer_color(graphic: Ptr, mut color: Color) {
    let u = unity();
    if u.graphic_get_canvas_renderer == 0 || u.canvas_renderer_set_color == 0 {
        return;
    }
    let cr = call!(u.graphic_get_canvas_renderer, fn(Ptr, Ptr) -> Ptr, graphic, NULL_MI);
    if !is_alive(cr) {
        return;
    }
    if u.canvas_renderer_color_injected {
        call!(u.canvas_renderer_set_color, fn(Ptr, *mut Color) -> (), cr, &mut color);
    } else {
        call!(u.canvas_renderer_set_color, fn(Ptr, Color) -> (), cr, color);
    }
}

