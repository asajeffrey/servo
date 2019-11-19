# A GStreamer plugin which runs servo

To build:
```
./mach build -r -p servo-gst-plugin
```

By default, gstreamer's plugin finder will complain about any libraries it finds that aren't
gstreamer plugins, so we need to have a directory just for plugins:
```
mkdir target/gstplugins
```

To install:
```
cp target/release/libgstservoplugin.* target/gstplugins
```

To run:
```
GST_PLUGIN_PATH=target/gstplugins \
  gst-launch-1.0 servosrc \
    ! video/x-raw,width=512,height=256,framerate=25/1 \
    ! queue \
    ! videoflip video-direction=vert \
    ! autovideosink
```

*Note*: killing the gstreamer pipeline with control-C sometimes locks up macOS to the point
of needing a power cycle. Killing the pipeline by closing the window seems to work.