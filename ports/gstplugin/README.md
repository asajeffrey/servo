# A GStreamer plugin which runs servo

## Build

```
./mach build -r -p servo-gst-plugin
```

## Install

By default, gstreamer's plugin finder will complain about any libraries it finds that aren't
gstreamer plugins, so we need to have a directory just for plugins:
```
mkdir target/gstplugins
```

To install:
```
cp target/release/libgstservoplugin.* target/gstplugins
```
## Run

```
GST_PLUGIN_PATH=target/gstplugins \
  gst-launch-1.0 servosrc \
    ! queue \
    ! videoflip video-direction=vert \
    ! autovideosink
```

*Note*: killing the gstreamer pipeline with control-C sometimes locks up macOS to the point
of needing a power cycle. Killing the pipeline by closing the window seems to work.

## Troubleshooting

First try:
```
GST_PLUGIN_PATH=target/gstplugins \
  gst-inspect-1.0 servosrc
```

If that doesn't work, try:
```
GST_PLUGIN_PATH=target/gstplugins \
  gst-inspect-1.0 target/gstplugins/libgstservoplugin.so
```

If you get reports about the plugin being blacklisted, remove the (global!) gstreamer cache, e.g. under Linux:
```
rm -r ~/.cache/gstreamer-1.0
```

If you get complaints about not being able to find libraries, set `LD_LIBRARY_PATH`, e.g. to use Servo's Linux gstreamer:
```
LD_LIBRARY_PATH=$PWD/support/linux/gstreamer/gst/lib
```

If you get complaints `cannot allocate memory in static TLS block` this is caused by gstreamer initializing threads using
the system alloc, which causes problems if those threads run Rust code that uses jemalloc. The fix is to preload the plugin:
```
LD_PRELOAD=$PWD/target/gstplugins/libgstservoplugin.so
```

You may need to set `GST_PLUGIN_SCANNER`, e.g. to use Servo's:
```
GST_PLUGIN_SCANNER=$PWD/support/linux/gstreamer/gst/libexec/gstreamer-1.0/gst-plugin-scanner
```

You may need to include other directories on the plugin search path, e.g. Servo's gstreamer:
```
GST_PLUGIN_PATH=$PWD/target/gstplugins/:$PWD/support/linux/gstreamer/gst/lib
```

Putting that all together:
```
GST_PLUGIN_SCANNER=$PWD/support/linux/gstreamer/gst/libexec/gstreamer-1.0/gst-plugin-scanner \
LD_PRELOAD=$PWD/target/gstplugins/libgstservoplugin.so \
GST_PLUGIN_PATH=$PWD/target/gstplugins/:$PWD/support/linux/gstreamer/gst/lib \
LD_LIBRARY_PATH=$PWD/support/linux/gstreamer/gst/lib \
  gst-launch-1.0 servosrc \
    ! queue \
    ! videoflip video-direction=vert \
    ! autovideosink
```