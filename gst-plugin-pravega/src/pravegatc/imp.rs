// A source that reads GStreamer buffers along with timestamps, as written by pravegasink.

use glib::subclass::prelude::*;
use gst::ClockTime;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst::{gst_error, gst_info, gst_log, gst_trace};
use gst_base::prelude::*;
use gst_base::subclass::prelude::*;

use std::convert::{TryInto, TryFrom};
use std::io::{BufReader, ErrorKind, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::u8;

use once_cell::sync::Lazy;

use pravega_client::client_factory::ClientFactory;
use pravega_client::byte_stream::ByteStreamReader;
use pravega_client_config::ClientConfigBuilder;
use pravega_client_shared::{Scope, Stream, Segment, ScopedSegment, StreamConfiguration, ScopedStream, Scaling, ScaleType};
use pravega_video::event_serde::EventReader;
use pravega_video::index::{IndexSearcher, get_index_stream_name};
use pravega_video::timestamp::PravegaTimestamp;
use pravega_video::utils;
use crate::seekable_take::SeekableTake;

const PROPERTY_NAME_STREAM: &str = "stream";
const PROPERTY_NAME_CONTROLLER: &str = "controller";
const PROPERTY_NAME_BUFFER_SIZE: &str = "buffer-size";
const PROPERTY_NAME_START_PTS_AT_ZERO: &str = "start-pts-at-zero";
const PROPERTY_NAME_START_MODE: &str = "start-mode";
const PROPERTY_NAME_END_MODE: &str = "end-mode";
const PROPERTY_NAME_START_TIMESTAMP: &str = "start-timestamp";
const PROPERTY_NAME_END_TIMESTAMP: &str = "end-timestamp";
const PROPERTY_NAME_START_UTC: &str = "start-utc";
const PROPERTY_NAME_END_UTC: &str = "end-utc";

// #[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Clone, Copy, glib::GEnum)]
// #[repr(u32)]
// #[genum(type_name = "GstStartMode")]
// pub enum StartMode {
//     #[genum(
//         name = "This element will not initiate a seek when starting. \
//                 Usually a pipeline will start with a seek to position 0, \
//                 in which case this would be equivalent to earliest.",
//         nick = "no-seek"
//     )]
//     NoSeek = 0,
//     #[genum(
//         name = "Start at the earliest available random-access point.",
//         nick = "earliest"
//     )]
//     Earliest = 1,
//     #[genum(
//         name = "Start at the most recent random-access point.",
//         nick = "latest"
//     )]
//     Latest = 2,
//     #[genum(
//         name = "Start at the random-access point on or immediately before \
//                 the specified start-timestamp or start-utc.",
//         nick = "timestamp"
//     )]
//     Timestamp = 3,
// }

// #[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Clone, Copy, glib::GEnum)]
// #[repr(u32)]
// #[genum(type_name = "GstEndMode")]
// pub enum EndMode {
//     #[genum(
//         name = "Do not stop until the stream has been sealed.",
//         nick = "unbounded"
//     )]
//     Unbounded = 0,
//     #[genum(
//         name = "Determine the last byte in the data stream when the pipeline starts. \
//                 Stop immediately after that byte has been emitted.",
//         nick = "latest"
//     )]
//     Latest = 1,
//     #[genum(
//         name = "Search the index for the last record when the pipeline starts. \
//                 Stop immediately before the located position.",
//         nick = "latest-indexed"
//     )]
//     LatestIndexed = 2,
//     #[genum(
//         name = "Search the index for the record on or immediately after \
//                 the specified end-timestamp or end-utc. \
//                 Stop immediately before the located position.",
//         nick = "timestamp"
//     )]
//     Timestamp = 3,
// }

const DEFAULT_CONTROLLER: &str = "127.0.0.1:9090";
const DEFAULT_BUFFER_SIZE: usize = 128*1024;
const DEFAULT_START_PTS_AT_ZERO: bool = false;
// const DEFAULT_START_MODE: StartMode = StartMode::NoSeek;
// const DEFAULT_END_MODE: EndMode = EndMode::Unbounded;
const DEFAULT_START_TIMESTAMP: u64 = 0;
const DEFAULT_END_TIMESTAMP: u64 = u64::MAX;

#[derive(Debug)]
struct Settings {
    scope: Option<String>,
    stream: Option<String>,
    controller: Option<String>,
    buffer_size: usize,
    // start_pts_at_zero: bool,
    // start_mode: StartMode,
    // end_mode: EndMode,
    // start_timestamp: u64,
    // end_timestamp: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            scope: None,
            stream: None,
            controller: Some(DEFAULT_CONTROLLER.to_owned()),
            buffer_size: DEFAULT_BUFFER_SIZE,
            // start_pts_at_zero: DEFAULT_START_PTS_AT_ZERO,
            // start_mode: DEFAULT_START_MODE,
            // end_mode: DEFAULT_END_MODE,
            // start_timestamp: DEFAULT_START_TIMESTAMP,
            // end_timestamp: DEFAULT_END_TIMESTAMP,
        }
    }
}

enum State {
    Stopped,
    Started {
        reader: Arc<Mutex<BufReader<SeekableTake<ByteStreamReader>>>>,
        index_searcher: Arc<Mutex<IndexSearcher<ByteStreamReader>>>,
    },
}

impl Default for State {
    fn default() -> State {
        State::Stopped
    }
}

pub struct PravegaTC {
    // settings: Mutex<Settings>,
    // state: Mutex<State>,
    srcpad: gst::Pad,
    sinkpad: gst::Pad,
}

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        "pravegatc",
        gst::DebugColorFlags::empty(),
        Some("Pravega Transaction Coordinator"),
    )
});

impl PravegaTC {
    // Called whenever a new buffer is passed to our sink pad. Here buffers should be processed and
    // whenever some output buffer is available have to push it out of the source pad.
    // Here we just pass through all buffers directly
    //
    // See the documentation of gst::Buffer and gst::BufferRef to see what can be done with
    // buffers.
    fn sink_chain(
        &self,
        pad: &gst::Pad,
        _element: &super::PravegaTC,
        buffer: gst::Buffer,
    ) -> Result<gst::FlowSuccess, gst::FlowError> {
        gst_log!(CAT, obj: pad, "Handling buffer {:?}", buffer);
        self.srcpad.push(buffer)
    }

    // Called whenever an event arrives on the sink pad. It has to be handled accordingly and in
    // most cases has to be either passed to Pad::event_default() on this pad for default handling,
    // or Pad::push_event() on all pads with the opposite direction for direct forwarding.
    // Here we just pass through all events directly to the source pad.
    //
    // See the documentation of gst::Event and gst::EventRef to see what can be done with
    // events, and especially the gst::EventView type for inspecting events.
    fn sink_event(&self, pad: &gst::Pad, _element: &super::PravegaTC, event: gst::Event) -> bool {
        gst_log!(CAT, obj: pad, "Handling event {:?}", event);
        self.srcpad.push_event(event)
    }

    // Called whenever a query is sent to the sink pad. It has to be answered if the element can
    // handle it, potentially by forwarding the query first to the peer pads of the pads with the
    // opposite direction, or false has to be returned. Default handling can be achieved with
    // Pad::query_default() on this pad and forwarding with Pad::peer_query() on the pads with the
    // opposite direction.
    // Here we just forward all queries directly to the source pad's peers.
    //
    // See the documentation of gst::Query and gst::QueryRef to see what can be done with
    // queries, and especially the gst::QueryView type for inspecting and modifying queries.
    fn sink_query(
        &self,
        pad: &gst::Pad,
        _element: &super::PravegaTC,
        query: &mut gst::QueryRef,
    ) -> bool {
        gst_log!(CAT, obj: pad, "Handling query {:?}", query);
        self.srcpad.peer_query(query)
    }

    // Called whenever an event arrives on the source pad. It has to be handled accordingly and in
    // most cases has to be either passed to Pad::event_default() on the same pad for default
    // handling, or Pad::push_event() on all pads with the opposite direction for direct
    // forwarding.
    // Here we just pass through all events directly to the sink pad.
    //
    // See the documentation of gst::Event and gst::EventRef to see what can be done with
    // events, and especially the gst::EventView type for inspecting events.
    fn src_event(&self, pad: &gst::Pad, _element: &super::PravegaTC, event: gst::Event) -> bool {
        gst_log!(CAT, obj: pad, "Handling event {:?}", event);
        self.sinkpad.push_event(event)
    }

    // Called whenever a query is sent to the source pad. It has to be answered if the element can
    // handle it, potentially by forwarding the query first to the peer pads of the pads with the
    // opposite direction, or false has to be returned. Default handling can be achieved with
    // Pad::query_default() on this pad and forwarding with Pad::peer_query() on the pads with the
    // opposite direction.
    // Here we just forward all queries directly to the sink pad's peers.
    //
    // See the documentation of gst::Query and gst::QueryRef to see what can be done with
    // queries, and especially the gst::QueryView type for inspecting and modifying queries.
    fn src_query(
        &self,
        pad: &gst::Pad,
        _element: &super::PravegaTC,
        query: &mut gst::QueryRef,
    ) -> bool {
        gst_log!(CAT, obj: pad, "Handling query {:?}", query);
        self.sinkpad.peer_query(query)
    }

    // fn set_stream(
    //     &self,
    //     element: &super::PravegaTC,
    //     stream: Option<String>,
    // ) -> Result<(), glib::Error> {
    //     let mut settings = self.settings.lock().unwrap();
    //     let (scope, stream) = match stream {
    //         Some(stream) => {
    //             let components: Vec<&str> = stream.split('/').collect();
    //             if components.len() != 2 {
    //                 return Err(glib::Error::new(
    //                     gst::URIError::BadUri,
    //                     format!("stream parameter '{}' is formatted incorrectly. It must be specified as scope/stream.", stream).as_str(),
    //                 ));
    //             }
    //             let scope = components[0].to_owned();
    //             let stream = components[1].to_owned();
    //             (Some(scope), Some(stream))
    //         }
    //         None => {
    //             gst_info!(CAT, obj: element, "Resetting `{}` to None", PROPERTY_NAME_STREAM);
    //             (None, None)
    //         }
    //     };
    //     settings.scope = scope;
    //     settings.stream = stream;
    //     Ok(())
    // }

    // fn set_controller(
    //     &self,
    //     _element: &super::PravegaTC,
    //     controller: Option<String>,
    // ) -> Result<(), glib::Error> {
    //     let mut settings = self.settings.lock().unwrap();
    //     settings.controller = controller;
    //     Ok(())
    // }
}

#[glib::object_subclass]
impl ObjectSubclass for PravegaTC {
    const NAME: &'static str = "PravegaTC";
    type Type = super::PravegaTC;
    type ParentType = gst::Element;

    // fn new() -> Self {
    //     pravega_video::tracing::init();
    //     Self {
    //         settings: Mutex::new(Default::default()),
    //         state: Mutex::new(Default::default()),
    //     }
    // }

    // Called when a new instance is to be created. We need to return an instance
    // of our struct here and also get the class struct passed in case it's needed
    fn with_class(klass: &Self::Class) -> Self {
        // Create our two pads from the templates that were registered with
        // the class and set all the functions on them.
        //
        // Each function is wrapped in catch_panic_pad_function(), which will
        // - Catch panics from the pad functions and instead of aborting the process
        //   it will simply convert them into an error message and poison the element
        //   instance
        // - Extract our Identity struct from the object instance and pass it to us
        //
        // Details about what each function is good for is next to each function definition
        let templ = klass.get_pad_template("sink").unwrap();
        let sinkpad = gst::Pad::builder_with_template(&templ, Some("sink"))
            .chain_function(|pad, parent, buffer| {
                PravegaTC::catch_panic_pad_function(
                    parent,
                    || Err(gst::FlowError::Error),
                    |identity, element| identity.sink_chain(pad, element, buffer),
                )
            })
            .event_function(|pad, parent, event| {
                PravegaTC::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.sink_event(pad, element, event),
                )
            })
            .query_function(|pad, parent, query| {
                PravegaTC::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.sink_query(pad, element, query),
                )
            })
            .build();

        let templ = klass.get_pad_template("src").unwrap();
        let srcpad = gst::Pad::builder_with_template(&templ, Some("src"))
            .event_function(|pad, parent, event| {
                PravegaTC::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.src_event(pad, element, event),
                )
            })
            .query_function(|pad, parent, query| {
                PravegaTC::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.src_query(pad, element, query),
                )
            })
            .build();

        // Return an instance of our struct and also include our debug category here.
        // The debug category will be used later whenever we need to put something
        // into the debug logs
        Self { srcpad, sinkpad }
    }
}

impl ObjectImpl for PravegaTC {
    // Called right after construction of a new instance
    fn constructed(&self, obj: &Self::Type) {
        // Call the parent class' ::constructed() implementation first
        self.parent_constructed(obj);

        // obj.set_format(gst::Format::Time);

        // Here we actually add the pads we created in Identity::new() to the
        // element so that GStreamer is aware of their existence.
        obj.add_pad(&self.sinkpad).unwrap();
        obj.add_pad(&self.srcpad).unwrap();
    }

    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: Lazy<Vec<glib::ParamSpec>> = Lazy::new(|| { vec![
            glib::ParamSpec::string(
                PROPERTY_NAME_STREAM,
                "Stream",
                "scope/stream",
                None,
                glib::ParamFlags::WRITABLE,
            ),
            glib::ParamSpec::string(
                PROPERTY_NAME_CONTROLLER,
                "Controller",
                "Pravega controller",
                None,
                glib::ParamFlags::WRITABLE,
            ),
            glib::ParamSpec::uint(
                PROPERTY_NAME_BUFFER_SIZE,
                "Buffer size",
                "Size of buffer in number of bytes",
                0,
                std::u32::MAX,
                DEFAULT_BUFFER_SIZE.try_into().unwrap(),
                glib::ParamFlags::WRITABLE,
            ),
            glib::ParamSpec::boolean(
                PROPERTY_NAME_START_PTS_AT_ZERO,
                "Start PTS at 0",
                "If true, the first buffer will have a PTS of 0. \
                If false, buffers will have a PTS equal to the raw timestamp stored in the Pravega stream \
                (nanoseconds since 1970-01-01 00:00 TAI International Atomic Time). \
                Use true when using sinks with sync=true such as an autoaudiosink. \
                Use false when using sinks with sync=false such as pravegasink.",
                DEFAULT_START_PTS_AT_ZERO,
                glib::ParamFlags::WRITABLE,
            ),
            // glib::ParamSpec::enum_(
            //     PROPERTY_NAME_START_MODE,
            //     "Start mode",
            //     "The position to start reading the stream at",
            //     StartMode::static_type(),
            //     DEFAULT_START_MODE as i32,
            //     glib::ParamFlags::WRITABLE,
            // ),
            // glib::ParamSpec::enum_(
            //     PROPERTY_NAME_END_MODE,
            //     "End mode",
            //     "The position to end reading the stream at",
            //     EndMode::static_type(),
            //     DEFAULT_END_MODE as i32,
            //     glib::ParamFlags::WRITABLE,
            // ),
            glib::ParamSpec::uint64(
                PROPERTY_NAME_START_TIMESTAMP,
                "Start timestamp",
                "If start-mode=timestamp, this is the timestamp at which to start, \
                in nanoseconds since 1970-01-01 00:00 TAI (International Atomic Time).",
                0,
                std::u64::MAX,
                DEFAULT_START_TIMESTAMP,
                glib::ParamFlags::WRITABLE,
            ),
            glib::ParamSpec::uint64(
                PROPERTY_NAME_END_TIMESTAMP,
                "End timestamp",
                "If end-mode=timestamp, this is the timestamp at which to stop, \
                in nanoseconds since 1970-01-01 00:00 TAI (International Atomic Time).",
                0,
                std::u64::MAX,
                DEFAULT_END_TIMESTAMP,
                glib::ParamFlags::WRITABLE,
            ),
            glib::ParamSpec::string(
                PROPERTY_NAME_START_UTC,
                "Start UTC",
                "If start-mode=utc, this is the timestamp at which to start, \
                in RFC 3339 format. For example: 2021-12-28T23:41:45.691Z",
                None,
                glib::ParamFlags::WRITABLE,
            ),
            glib::ParamSpec::string(
                PROPERTY_NAME_END_UTC,
                "End UTC",
                "If end-mode=utc, this is the timestamp at which to stop, \
                in RFC 3339 format. For example: 2021-12-28T23:41:45.691Z",
                None,
                glib::ParamFlags::WRITABLE,
            ),        
        ]});
        PROPERTIES.as_ref()
    }

    // TODO: On error, should set flag that will cause element to fail.
    fn set_property(
        &self,
        obj: &Self::Type,
        _id: usize,
        value: &glib::Value,
        pspec: &glib::ParamSpec,
    ) {
        match pspec.get_name() {
            // PROPERTY_NAME_STREAM => {
            //     let res = match value.get::<String>() {
            //         Ok(Some(stream)) => self.set_stream(&obj, Some(stream)),
            //         Ok(None) => self.set_stream(&obj, None),
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_STREAM, err);
            //     }
            // },
            // PROPERTY_NAME_CONTROLLER => {
            //     let res = match value.get::<String>() {
            //         Ok(controller) => self.set_controller(&obj, controller),
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_CONTROLLER, err);
            //     }
            // },
            // PROPERTY_NAME_BUFFER_SIZE => {
            //     let res: Result<(), glib::Error> = match value.get::<u32>() {
            //         Ok(buffer_size) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             settings.buffer_size = buffer_size.unwrap_or_default().try_into().unwrap_or_default();
            //             Ok(())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_BUFFER_SIZE, err);
            //     }
            // },
            // PROPERTY_NAME_START_PTS_AT_ZERO => {
            //     let res: Result<(), glib::Error> = match value.get::<bool>() {
            //         Ok(start_pts_at_zero) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             settings.start_pts_at_zero = start_pts_at_zero.unwrap_or_default();
            //             Ok(())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property {}: {}", PROPERTY_NAME_START_PTS_AT_ZERO, err);
            //     }
            // },
            // PROPERTY_NAME_START_MODE => {
            //     let res: Result<(), glib::Error> = match value.get::<StartMode>() {
            //         Ok(start_mode) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             settings.start_mode = start_mode.unwrap();
            //             Ok(())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_START_MODE, err);
            //     }
            // },
            // PROPERTY_NAME_END_MODE => {
            //     let res: Result<(), glib::Error> = match value.get::<EndMode>() {
            //         Ok(end_mode) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             settings.end_mode = end_mode.unwrap();
            //             Ok(())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_END_MODE, err);
            //     }
            // },
            // PROPERTY_NAME_START_TIMESTAMP => {
            //     let res: Result<(), glib::Error> = match value.get::<u64>() {
            //         Ok(start_timestamp) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             settings.start_timestamp = start_timestamp.unwrap_or_default().try_into().unwrap_or_default();
            //             Ok(())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_START_TIMESTAMP, err);
            //     }
            // },
            // PROPERTY_NAME_END_TIMESTAMP => {
            //     let res: Result<(), glib::Error> = match value.get::<u64>() {
            //         Ok(end_timestamp) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             settings.end_timestamp = end_timestamp.unwrap_or_default().try_into().unwrap_or_default();
            //             Ok(())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_END_TIMESTAMP, err);
            //     }
            // },
            // PROPERTY_NAME_START_UTC => {
            //     let res = match value.get::<String>() {
            //         Ok(start_utc) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             let timestamp = PravegaTimestamp::try_from(start_utc);
            //             timestamp.map(|t| settings.start_timestamp = t.nanoseconds().unwrap())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_START_UTC, err);
            //     }
            // },
            // PROPERTY_NAME_END_UTC => {
            //     let res = match value.get::<String>() {
            //         Ok(end_utc) => {
            //             let mut settings = self.settings.lock().unwrap();
            //             let timestamp = PravegaTimestamp::try_from(end_utc);
            //             timestamp.map(|t| settings.end_timestamp = t.nanoseconds().unwrap())
            //         },
            //         Err(_) => unreachable!("type checked upstream"),
            //     };
            //     if let Err(err) = res {
            //         gst_error!(CAT, obj: obj, "Failed to set property `{}`: {}", PROPERTY_NAME_END_UTC, err);
            //     }
            // },
        _ => unimplemented!(),
        };
    }
}

impl ElementImpl for PravegaTC {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                "Pravega Transaction Coordinator",
                "Source/Pravega",
                "Provides failure recovery for pipelines",
                "Claudio Fahey <claudio.fahey@dell.com>",
                )
        });
        Some(&*ELEMENT_METADATA)
    }

    // Create and add pad templates for our sink and source pad. These
    // are later used for actually creating the pads and beforehand
    // already provide information to GStreamer about all possible
    // pads that could exist for this type.
    //
    // Actual instances can create pads based on those pad templates
    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
            // Our element can accept any possible caps on both pads
            let caps = gst::Caps::new_any();
            let src_pad_template = gst::PadTemplate::new(
                "src",
                gst::PadDirection::Src,
                gst::PadPresence::Always,
                &caps,
            )
            .unwrap();

            let sink_pad_template = gst::PadTemplate::new(
                "sink",
                gst::PadDirection::Sink,
                gst::PadPresence::Always,
                &caps,
            )
            .unwrap();

            vec![src_pad_template, sink_pad_template]
        });
        PAD_TEMPLATES.as_ref()
    }

    // Called whenever the state of the element should be changed. This allows for
    // starting up the element, allocating/deallocating resources or shutting down
    // the element again.
    fn change_state(
        &self,
        element: &Self::Type,
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        gst_trace!(CAT, obj: element, "Changing state {:?}", transition);

        // Call the parent class' implementation of ::change_state()
        self.parent_change_state(element, transition)
    }
}
