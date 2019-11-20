/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::logging::CATEGORY;

use crossbeam_channel::Receiver;
use crossbeam_channel::Sender;

use euclid::Point2D;
use euclid::Rect;
use euclid::Scale;
use euclid::Size2D;

use glib::glib_bool_error;
use glib::glib_object_impl;
use glib::glib_object_subclass;
use glib::object::Cast;
use glib::subclass::object::ObjectImpl;
use glib::subclass::object::ObjectImplExt;
use glib::subclass::simple::ClassStruct;
use glib::subclass::types::ObjectSubclass;
use gstreamer::gst_element_error;
use gstreamer::gst_loggable_error;
use gstreamer::subclass::element::ElementClassSubclassExt;
use gstreamer::subclass::element::ElementImpl;
use gstreamer::subclass::ElementInstanceStruct;
use gstreamer::BufferRef;
use gstreamer::Caps;
use gstreamer::CoreError;
use gstreamer::ErrorMessage;
use gstreamer::FlowError;
use gstreamer::FlowSuccess;
use gstreamer::Format;
use gstreamer::Fraction;
use gstreamer::FractionRange;
use gstreamer::IntRange;
use gstreamer::LoggableError;
use gstreamer::PadDirection;
use gstreamer::PadPresence;
use gstreamer::PadTemplate;
use gstreamer_base::subclass::base_src::BaseSrcImpl;
use gstreamer_base::BaseSrc;
use gstreamer_base::BaseSrcExt;
use gstreamer_video::VideoFormat;
use gstreamer_video::VideoFrameRef;
use gstreamer_video::VideoInfo;

use log::debug;
use log::info;

use servo::compositing::windowing::AnimationState;
use servo::compositing::windowing::EmbedderCoordinates;
use servo::compositing::windowing::EmbedderMethods;
use servo::compositing::windowing::WindowEvent;
use servo::compositing::windowing::WindowMethods;
use servo::embedder_traits::EventLoopWaker;
use servo::msg::constellation_msg::TopLevelBrowsingContextId;
use servo::servo_url::ServoUrl;
use servo::webrender_api::units::DevicePixel;
use servo::Servo;

use sparkle::gl;
use sparkle::gl::types::GLenum;
use sparkle::gl::types::GLint;
use sparkle::gl::types::GLsizei;
use sparkle::gl::types::GLuint;
use sparkle::gl::Gl;

use surfman::platform::generic::universal::context::Context;
use surfman::platform::generic::universal::device::Device;
use surfman::SurfaceAccess;
use surfman::SurfaceType;

use surfman_chains::SwapChain;
use surfman_chains_api::SwapChainAPI;

use std::cell::RefCell;
use std::mem;
use std::ptr;
use std::rc::Rc;
use std::sync::Mutex;
use std::thread;

pub struct ServoSrc {
    sender: Sender<ServoSrcMsg>,
    swap_chain: SwapChain,
    info: Mutex<Option<VideoInfo>>,
}

struct ServoSrcGfx {
    device: Device,
    context: Context,
    gl: Rc<Gl>,
    read_fbo: GLuint,
    draw_fbo: GLuint,
    draw_texture: GLuint,
    draw_size: Size2D<i32, DevicePixel>,
    draw_target: GLuint,
}

impl ServoSrcGfx {
    fn new() -> ServoSrcGfx {
        let version = surfman::GLVersion { major: 4, minor: 3 };
        let flags = surfman::ContextAttributeFlags::ALPHA;
        let attributes = surfman::ContextAttributes { version, flags };

        let connection = surfman::Connection::new().expect("Failed to create connection");
        let adapter = surfman::Adapter::default().expect("Failed to create adapter");
        let mut device =
            surfman::Device::new(&connection, &adapter).expect("Failed to create device");
        let descriptor = device
            .create_context_descriptor(&attributes)
            .expect("Failed to create descriptor");
        let context = device
            .create_context(&descriptor)
            .expect("Failed to create context");
        let mut device = Device::Hardware(device);
        let mut context = Context::Hardware(context);
        let gl = Gl::gl_fns(gl::ffi_gl::Gl::load_with(|s| {
            device.get_proc_address(&context, s)
        }));

        device.make_context_current(&context).unwrap();

        let size = Size2D::new(512, 512);
        let surface_type = SurfaceType::Generic { size };
        let surface = device
            .create_surface(&mut context, SurfaceAccess::GPUCPU, &surface_type)
            .expect("Failed to create surface");
        device
            .bind_surface_to_context(&mut context, surface)
            .expect("Failed to bind surface");

        let read_fbo = gl.gen_framebuffers(1)[0];
        let draw_fbo = gl.gen_framebuffers(1)[0];
        let draw_texture = gl.gen_textures(1)[0];
        let draw_size = Size2D::from_untyped(size);
        let draw_target = device.surface_gl_texture_target();

        gl.bind_texture(draw_target, draw_texture);
        debug_assert_eq!(gl.get_error(), gl::NO_ERROR);

        gl.tex_image_2d(
            draw_target,
            0,
            gl::RGBA as i32,
            draw_size.width,
            draw_size.height,
            0,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            None,
        );
        debug_assert_eq!(gl.get_error(), gl::NO_ERROR);

        gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, draw_fbo);
        debug_assert_eq!(gl.get_error(), gl::NO_ERROR);

        gl.framebuffer_texture_2d(
            gl::DRAW_FRAMEBUFFER,
            gl::COLOR_ATTACHMENT0,
            draw_target,
            draw_texture,
            0,
        );
        debug_assert_eq!(gl.get_error(), gl::NO_ERROR);
        debug_assert_eq!(
            (gl.check_framebuffer_status(gl::FRAMEBUFFER), gl.get_error()),
            (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
        );

        device.make_no_context_current().unwrap();

        Self {
            device,
            context,
            gl,
            read_fbo,
            draw_fbo,
            draw_texture,
            draw_target,
            draw_size,
        }
    }
}

impl Drop for ServoSrcGfx {
    fn drop(&mut self) {
        let _ = self.device.destroy_context(&mut self.context);
    }
}

thread_local! {
    static GFX: RefCell<ServoSrcGfx> = RefCell::new(ServoSrcGfx::new());
}

#[derive(Debug)]
enum ServoSrcMsg {
    GetSwapChain(Sender<SwapChain>),
    Resize(Size2D<i32, DevicePixel>),
    Heartbeat,
    Quit,
}

const DEFAULT_URL: &'static str =
    "https://rawcdn.githack.com/mrdoob/three.js/r105/examples/webgl_animation_cloth.html";

struct ServoThread {
    receiver: Receiver<ServoSrcMsg>,
    swap_chain: SwapChain,
    servo: Servo<ServoSrcWindow>,
}

impl ServoThread {
    fn new(receiver: Receiver<ServoSrcMsg>) -> Self {
        let embedder = Box::new(ServoSrcEmbedder);
        let window = Rc::new(ServoSrcWindow::new());
        let swap_chain = window.swap_chain.clone();
        let servo = Servo::new(embedder, window);
        Self {
            receiver,
            swap_chain,
            servo,
        }
    }

    fn run(&mut self) {
        self.new_browser();
        while let Ok(msg) = self.receiver.recv() {
            debug!("Servo thread handling message {:?}", msg);
            match msg {
                ServoSrcMsg::GetSwapChain(sender) => sender
                    .send(self.swap_chain.clone())
                    .expect("Failed to send swap chain"),
                ServoSrcMsg::Resize(size) => self.resize(size),
                ServoSrcMsg::Heartbeat => self.servo.handle_events(vec![]),
                ServoSrcMsg::Quit => break,
            }
        }
        self.servo.handle_events(vec![WindowEvent::Quit]);
    }

    fn new_browser(&mut self) {
        let id = TopLevelBrowsingContextId::new();
        let url = ServoUrl::parse(DEFAULT_URL).unwrap();
        self.servo
            .handle_events(vec![WindowEvent::NewBrowser(url, id)]);
    }

    fn resize(&mut self, size: Size2D<i32, DevicePixel>) {
        GFX.with(|gfx| {
            let mut gfx = gfx.borrow_mut();
            let gfx = &mut *gfx;
            let _ = gfx.device.make_context_current(&mut gfx.context);
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            self.swap_chain
                .resize_TMP(
                    &mut gfx.device,
                    &mut gfx.context,
                    size.to_untyped(),
                    &mut gfx.gl,
                )
                .expect("Failed to resize");
            let _ = gfx.device.make_context_current(&mut gfx.context);
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            self.swap_chain
                .clear_surface(&mut gfx.device, &mut gfx.context, &gfx.gl)
                .expect("Failed to clear");
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            gfx.gl.viewport(0, 0, size.width, size.height);
            let fbo = gfx
                .device
                .context_surface_info(&gfx.context)
                .expect("Failed to get context info")
                .expect("Failed to get context info")
                .framebuffer_object;
            gfx.gl.bind_framebuffer(gl::FRAMEBUFFER, fbo);
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            let _ = gfx.device.make_no_context_current();
        });
        self.servo.handle_events(vec![WindowEvent::Resize]);
    }
}

impl Drop for ServoThread {
    fn drop(&mut self) {
        GFX.with(|gfx| {
            let mut gfx = gfx.borrow_mut();
            let gfx = &mut *gfx;
            self.swap_chain
                .destroy(&mut gfx.device, &mut gfx.context)
                .expect("Failed to destroy swap chain")
        })
    }
}

struct ServoSrcEmbedder;

impl EmbedderMethods for ServoSrcEmbedder {
    fn create_event_loop_waker(&mut self) -> Box<dyn EventLoopWaker> {
        Box::new(ServoSrcEmbedder)
    }
}

impl EventLoopWaker for ServoSrcEmbedder {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(ServoSrcEmbedder)
    }

    fn wake(&self) {}
}

struct ServoSrcWindow {
    swap_chain: SwapChain,
    gl: Rc<dyn gleam::gl::Gl>,
}

impl ServoSrcWindow {
    fn new() -> Self {
        GFX.with(|gfx| {
            let mut gfx = gfx.borrow_mut();
            let gfx = &mut *gfx;
            let _ = gfx.device.make_context_current(&mut gfx.context);
            let access = SurfaceAccess::GPUCPU;
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            let swap_chain = SwapChain::create_attached(&mut gfx.device, &mut gfx.context, access)
                .expect("Failed to create swap chain");
            let fbo = gfx
                .device
                .context_surface_info(&gfx.context)
                .expect("Failed to get context info")
                .expect("Failed to get context info")
                .framebuffer_object;
            gfx.gl.bind_framebuffer(gl::FRAMEBUFFER, fbo);
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            let gl = unsafe {
                gleam::gl::GlFns::load_with(|s| gfx.device.get_proc_address(&gfx.context, s))
            };
            let _ = gfx.device.make_no_context_current();
            Self { swap_chain, gl }
        })
    }
}

impl WindowMethods for ServoSrcWindow {
    fn present(&self) {
        GFX.with(|gfx| {
            debug!("EMBEDDER present");
            let mut gfx = gfx.borrow_mut();
            let gfx = &mut *gfx;
            let _ = gfx.device.make_context_current(&mut gfx.context);
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            let _ = self
                .swap_chain
                .swap_buffers(&mut gfx.device, &mut gfx.context);
            let _ = gfx.device.make_context_current(&mut gfx.context);
            let fbo = gfx
                .device
                .context_surface_info(&gfx.context)
                .expect("Failed to get context info")
                .expect("Failed to get context info")
                .framebuffer_object;
            gfx.gl.bind_framebuffer(gl::FRAMEBUFFER, fbo);
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
            let _ = gfx.device.make_no_context_current();
        })
    }

    fn make_gl_context_current(&self) {
        GFX.with(|gfx| {
            debug!("EMBEDDER make_context_current");
            let mut gfx = gfx.borrow_mut();
            let gfx = &mut *gfx;
            let _ = gfx.device.make_context_current(&gfx.context);
            debug_assert_eq!(
                (
                    gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                    gfx.gl.get_error()
                ),
                (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
            );
        })
    }

    fn gl(&self) -> Rc<dyn gleam::gl::Gl> {
        self.gl.clone()
    }

    fn get_coordinates(&self) -> EmbedderCoordinates {
        GFX.with(|gfx| {
            debug!("EMBEDDER get_coordinates");
            let mut gfx = gfx.borrow_mut();
            let gfx = &mut *gfx;
            let size = gfx
                .device
                .context_surface_info(&gfx.context)
                .unwrap()
                .unwrap()
                .size;
            let size = Size2D::from_untyped(size);
            let origin = Point2D::origin();
            EmbedderCoordinates {
                hidpi_factor: Scale::new(1.0),
                screen: size,
                screen_avail: size,
                window: (size, origin),
                framebuffer: size,
                viewport: Rect::new(origin, size),
            }
        })
    }

    fn set_animation_state(&self, _: AnimationState) {}

    fn get_gl_context(&self) -> servo_media::player::context::GlContext {
        servo_media::player::context::GlContext::Unknown
    }

    fn get_native_display(&self) -> servo_media::player::context::NativeDisplay {
        servo_media::player::context::NativeDisplay::Unknown
    }

    fn get_gl_api(&self) -> servo_media::player::context::GlApi {
        servo_media::player::context::GlApi::OpenGL3
    }
}

impl ObjectSubclass for ServoSrc {
    const NAME: &'static str = "ServoSrc";
    // gstreamer-gl doesn't have support for GLBaseSrc yet
    // https://gitlab.freedesktop.org/gstreamer/gstreamer-rs/issues/219
    type ParentType = BaseSrc;
    type Instance = ElementInstanceStruct<Self>;
    type Class = ClassStruct<Self>;

    fn new() -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        thread::spawn(move || ServoThread::new(receiver).run());
        let (acks, ackr) = crossbeam_channel::bounded(1);
        let _ = sender.send(ServoSrcMsg::GetSwapChain(acks));
        let swap_chain = ackr.recv().expect("Failed to get swap chain");
        let info = Mutex::new(None);
        Self {
            sender,
            swap_chain,
            info,
        }
    }

    fn class_init(klass: &mut ClassStruct<Self>) {
        klass.set_metadata(
            "Servo as a gstreamer src",
            "Filter/Effect/Converter/Video",
            "The Servo web browser",
            env!("CARGO_PKG_AUTHORS"),
        );

        let src_caps = Caps::new_simple(
            "video/x-raw",
            &[
                ("format", &VideoFormat::Bgrx.to_string()),
                ("width", &IntRange::<i32>::new(1, std::i32::MAX)),
                ("height", &IntRange::<i32>::new(1, std::i32::MAX)),
                (
                    "framerate",
                    &FractionRange::new(
                        Fraction::new(1, std::i32::MAX),
                        Fraction::new(std::i32::MAX, 1),
                    ),
                ),
            ],
        );
        let src_pad_template =
            PadTemplate::new("src", PadDirection::Src, PadPresence::Always, &src_caps).unwrap();
        klass.add_pad_template(src_pad_template);
    }

    glib_object_subclass!();
}

impl ObjectImpl for ServoSrc {
    glib_object_impl!();

    fn constructed(&self, obj: &glib::Object) {
        self.parent_constructed(obj);
        let basesrc = obj.downcast_ref::<BaseSrc>().unwrap();
        basesrc.set_live(true);
        basesrc.set_format(Format::Time);
    }
}

impl ElementImpl for ServoSrc {}

impl BaseSrcImpl for ServoSrc {
    fn set_caps(&self, _src: &BaseSrc, outcaps: &Caps) -> Result<(), LoggableError> {
        let info = VideoInfo::from_caps(outcaps)
            .ok_or_else(|| gst_loggable_error!(CATEGORY, "Failed to get video info"))?;
        let size = Size2D::new(info.width(), info.height()).to_i32();
        debug!("Setting servosrc buffer size to {}", size,);
        self.sender
            .send(ServoSrcMsg::Resize(size))
            .map_err(|_| gst_loggable_error!(CATEGORY, "Failed to send video info"))?;
        *self.info.lock().unwrap() = Some(info);
        Ok(())
    }

    fn start(&self, _src: &BaseSrc) -> Result<(), ErrorMessage> {
        info!("Starting");
        Ok(())
    }

    fn stop(&self, _src: &BaseSrc) -> Result<(), ErrorMessage> {
        info!("Starting");
        let _ = self.sender.send(ServoSrcMsg::Quit);
        Ok(())
    }

    fn fill(
        &self,
        src: &BaseSrc,
        _offset: u64,
        _length: u32,
        buffer: &mut BufferRef,
    ) -> Result<FlowSuccess, FlowError> {
        let guard = self.info.lock().map_err(|_| {
            gst_element_error!(src, CoreError::Negotiation, ["Lock poisoned"]);
            FlowError::NotNegotiated
        })?;
        let info = guard.as_ref().ok_or_else(|| {
            gst_element_error!(src, CoreError::Negotiation, ["Caps not set yet"]);
            FlowError::NotNegotiated
        })?;
        let mut frame = VideoFrameRef::from_buffer_ref_writable(buffer, info).ok_or_else(|| {
            gst_element_error!(
                src,
                CoreError::Failed,
                ["Failed to map output buffer writable"]
            );
            FlowError::Error
        })?;
        let frame_size = Size2D::new(frame.height(), frame.width()).to_i32();
        let data = frame.plane_data_mut(0).unwrap();

        GFX.with(|gfx| {
            let mut gfx = gfx.borrow_mut();
            let gfx = &mut *gfx;

            if let Some(surface) = self.swap_chain.take_surface() {
                gfx.device.make_context_current(&mut gfx.context);
                debug_assert_eq!(gfx.gl.get_error(), gl::NO_ERROR);

                let surface_size = gfx.device.surface_info(&surface).size;

                let surface_texture = gfx
                    .device
                    .create_surface_texture(&mut gfx.context, surface)
                    .unwrap();
                let texture = surface_texture.gl_texture();
                /*
                gfx.gl.bind_texture(gfx.device.surface_gl_texture_target(), texture);
                                    debug_assert_eq!(
                                        (
                                            gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                                            gfx.gl.get_error()
                                        ),
                                        (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                                    );
                if let Gl::Gl(ref gl) = *gfx.gl {
                unsafe { gl.GetTexImage(gfx.device.surface_gl_texture_target(), 0, gl::RGBA, gl::UNSIGNED_BYTE, data.as_mut_ptr() as _) ;}
                } else {
                panic!("EGL???");
                }
                                    debug_assert_eq!(
                                        (
                                            gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                                            gfx.gl.get_error()
                                        ),
                                        (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                                    );
                */

                // gfx.gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, gfx.draw_fbo);
                let draw_fbo = gfx
                    .device
                    .context_surface_info(&gfx.context)
                    .unwrap()
                    .unwrap()
                    .framebuffer_object;
                // let draw_fbo = gfx.draw_fbo;
                gfx.gl.bind_framebuffer(gl::DRAW_FRAMEBUFFER, draw_fbo);
                gfx.gl.bind_framebuffer(gl::READ_FRAMEBUFFER, gfx.read_fbo);
                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );

                if frame_size != gfx.draw_size {
                    panic!("Not there yet");
                    gfx.gl.bind_texture(gfx.draw_target, gfx.draw_texture);
                    debug_assert_eq!(
                        (
                            gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                            gfx.gl.get_error()
                        ),
                        (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                    );

                    gfx.gl.tex_image_2d(
                        gfx.draw_target,
                        0,
                        gl::RGBA as i32,
                        frame_size.width,
                        frame_size.height,
                        0,
                        gl::RGBA,
                        gl::UNSIGNED_BYTE,
                        None,
                    );
                    debug_assert_eq!(
                        (
                            gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                            gfx.gl.get_error()
                        ),
                        (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                    );
                    gfx.draw_size = frame_size;
                }

                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );
                /*
                                gfx.gl.framebuffer_texture_2d(
                                    gl::DRAW_FRAMEBUFFER,
                                    gl::COLOR_ATTACHMENT0,
                                    gfx.draw_target,
                                    gfx.draw_texture,
                                    0,
                                );
                                debug_assert_eq!(
                                    (
                                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                                        gfx.gl.get_error()
                                    ),
                                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                                );
                */
                gfx.gl.framebuffer_texture_2d(
                    gl::READ_FRAMEBUFFER,
                    gl::COLOR_ATTACHMENT0,
                    gfx.device.surface_gl_texture_target(),
                    texture,
                    0,
                );
                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );

                gfx.gl.clear_color(0.2, 0.3, 0.3, 1.0);
                gfx.gl.clear(gl::COLOR_BUFFER_BIT);
                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );

                gfx.gl.blit_framebuffer(
                    0,
                    0,
                    surface_size.width,
                    surface_size.height,
                    0,
                    0,
                    frame_size.width,
                    frame_size.height,
                    gl::COLOR_BUFFER_BIT,
                    gl::NEAREST,
                );
                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );

                gfx.gl.bind_framebuffer(gl::READ_FRAMEBUFFER, draw_fbo);
                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );

                // TODO: use GL memory to avoid readback
                gfx.gl.read_pixels_into_buffer(
                    0,
                    0,
                    frame_size.width,
                    frame_size.height,
                    gl::BGRA,
                    gl::UNSIGNED_BYTE,
                    data,
                );
                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );

                debug!("Read pixels {:?}", &data[..127]);

                let surface = gfx
                    .device
                    .destroy_surface_texture(&mut gfx.context, surface_texture)
                    .unwrap();
                self.swap_chain.recycle_surface(surface);
                debug_assert_eq!(
                    (
                        gfx.gl.check_framebuffer_status(gl::FRAMEBUFFER),
                        gfx.gl.get_error()
                    ),
                    (gl::FRAMEBUFFER_COMPLETE, gl::NO_ERROR)
                );
                gfx.device.make_no_context_current().unwrap();
            }
        });
        let _ = self.sender.send(ServoSrcMsg::Heartbeat);
        Ok(FlowSuccess::Ok)
    }
}
