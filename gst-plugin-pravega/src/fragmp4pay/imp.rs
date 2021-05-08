//
// Copyright (c) Dell Inc., or its subsidiaries. All Rights Reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//

use glib::subclass::prelude::*;
use gst::ClockTime;
use gst::prelude::*;
use gst::subclass::prelude::*;
#[allow(unused_imports)]
use gst::{gst_debug, gst_error, gst_info, gst_log, gst_trace};
use once_cell::sync::Lazy;
use std::convert::TryInto;
use std::sync::Mutex;

const ELEMENT_CLASS_NAME: &str = "FragMp4Pay";
const ELEMENT_LONG_NAME: &str = "Fragmented MP4 Payloader";
const DEBUG_CATEGORY: &str = "fragmp4pay";

struct StartedState {
    // Atoms in init sequence that must be repeated at each key frame.
    ftype_atom: Vec<u8>,
    moov_atom: Vec<u8>,
    // Atoms that must be buffered and pushed as a single buffer.
    moof_atom: Vec<u8>,
    mdat_size: u64,
    mdat_atom: Vec<u8>,
    mdat_first_pts: ClockTime,
    mdat_buffer_flags: gst::BufferFlags,
}

enum State {
    Started {
        state: StartedState,
    }
}

impl Default for State {
    fn default() -> State {
        State::Started {
            state: StartedState {
                ftype_atom: Vec::new(),
                moov_atom: Vec::new(),
                moof_atom: Vec::new(),
                mdat_size: 0,
                mdat_atom: Vec::new(),
                mdat_first_pts: ClockTime::none(),
                mdat_buffer_flags: gst::BufferFlags::empty(),
                }
        }
    }
}

pub struct FragMp4Pay {
    state: Mutex<State>,
    srcpad: gst::Pad,
    sinkpad: gst::Pad,
}

static CAT: Lazy<gst::DebugCategory> = Lazy::new(|| {
    gst::DebugCategory::new(
        DEBUG_CATEGORY,
        gst::DebugColorFlags::empty(),
        Some(ELEMENT_LONG_NAME),
    )
});

impl FragMp4Pay {
    fn sink_chain(
        &self,
        pad: &gst::Pad,
        element: &super::FragMp4Pay,
        buffer: gst::Buffer,
    ) -> Result<gst::FlowSuccess, gst::FlowError> {
        gst_debug!(CAT, obj: pad, "Handling buffer {:?}", buffer);

        // let settings = self.settings.lock().unwrap();
        let mut state = self.state.lock().unwrap();

        let state = match *state {
            State::Started {
                ref mut state,
                ..
            } => state,
        };

        {
            let map = buffer.map_readable().map_err(|_| {
                gst::element_error!(element, gst::CoreError::Failed, ["Failed to map buffer"]);
                gst::FlowError::Error
            })?;
            let input_buf = map.as_ref();

            const ATOM_TYPE_FTYPE: u32 = 1718909296;
            const ATOM_TYPE_MOOV: u32 = 1836019574;
            const ATOM_TYPE_MOOF: u32 = 1836019558;
            const ATOM_TYPE_MDAT: u32 = 1835295092;

            if state.mdat_size > 0 {
                // We expect the mdat body.
                if state.mdat_first_pts.is_none() {
                    state.mdat_first_pts = buffer.get_pts();
                }
                state.mdat_atom.extend_from_slice(input_buf);
                if (state.mdat_atom.len() as u64) < state.mdat_size {
                    gst_debug!(CAT, obj: pad, "incomplete mdat_atom=[{}], mdat_first_pts={}", state.mdat_atom.len(), state.mdat_first_pts);
                    Ok(gst::FlowSuccess::Ok)
                } else {
                    assert_eq!(state.mdat_atom.len() as u64, state.mdat_size);
                    gst_debug!(CAT, obj: pad, "complete mdat_atom=[{}], mdat_first_pts={}", state.mdat_atom.len(), state.mdat_first_pts);

                    // We have the complete mdat atom. Push everything downstream.
                    // TODO: Only send ftype, moov on key frames.
                    let output_buf_len = state.ftype_atom.len() + state.moov_atom.len() + state.moof_atom.len() + state.mdat_atom.len();
                    let mut gst_buffer = gst::Buffer::with_size(output_buf_len).unwrap();
                    {
                        let buffer_ref = gst_buffer.get_mut().unwrap();
                        buffer_ref.set_pts(state.mdat_first_pts);
                        buffer_ref.set_flags(state.mdat_buffer_flags);
                        let mut buffer_map = buffer_ref.map_writable().unwrap();
                        let slice = buffer_map.as_mut_slice();
                        let mut pos = 0;
                        slice[pos..pos+state.ftype_atom.len()].copy_from_slice(&state.ftype_atom);
                        pos += state.ftype_atom.len();
                        slice[pos..pos+state.moov_atom.len()].copy_from_slice(&state.moov_atom);
                        pos += state.moov_atom.len();
                        slice[pos..pos+state.moof_atom.len()].copy_from_slice(&state.moof_atom);
                        pos += state.moof_atom.len();
                        slice[pos..pos+state.mdat_atom.len()].copy_from_slice(&state.mdat_atom);
                    }
                    state.mdat_size = 0;
                    state.mdat_atom.clear();
                    self.srcpad.push(gst_buffer)
                }
            } else {
                // We expect an atom.
                let atom_size = u32::from_be_bytes(input_buf[0..4].try_into().unwrap());
                let atom_type = u32::from_be_bytes(input_buf[4..8].try_into().unwrap());
                gst_debug!(CAT, obj: pad, "atom_size={}, atom_type={}", atom_size, atom_type);
                match atom_type {
                    ATOM_TYPE_FTYPE => {
                        state.ftype_atom.clear();
                        state.ftype_atom.extend_from_slice(input_buf);
                        gst_debug!(CAT, obj: pad, "ftype_atom={:?}", state.ftype_atom);
                    },
                    ATOM_TYPE_MOOV => {
                        state.moov_atom.clear();
                        state.moov_atom.extend_from_slice(input_buf);
                        gst_debug!(CAT, obj: pad, "moov_atom={:?}", state.moov_atom);
                    },
                    ATOM_TYPE_MOOF => {
                        state.moof_atom.clear();
                        state.moof_atom.extend_from_slice(input_buf);
                        gst_debug!(CAT, obj: pad, "moof_atom={:?}", state.moof_atom);
                    },
                    ATOM_TYPE_MDAT => {
                        state.mdat_size = atom_size as u64;
                        state.mdat_first_pts = buffer.get_pts();
                        state.mdat_buffer_flags = buffer.get_flags();
                        state.mdat_atom.clear();
                        state.mdat_atom.extend_from_slice(input_buf);
                        gst_debug!(CAT, obj: pad, "new mdat_atom={:?}, mdat_first_pts={}, mdat_buffer_flags={:?}",
                        state.mdat_atom, state.mdat_first_pts, state.mdat_buffer_flags);
                    },
                    _ => {},
                }
                Ok(gst::FlowSuccess::Ok)
            }
        }
    }

    fn sink_event(&self, pad: &gst::Pad, _element: &super::FragMp4Pay, event: gst::Event) -> bool {
        gst_log!(CAT, obj: pad, "Handling event {:?}", event);
        self.srcpad.push_event(event)
    }

    fn sink_query(
        &self,
        pad: &gst::Pad,
        _element: &super::FragMp4Pay,
        query: &mut gst::QueryRef,
    ) -> bool {
        gst_log!(CAT, obj: pad, "Handling query {:?}", query);
        self.srcpad.peer_query(query)
    }

    fn src_event(&self, pad: &gst::Pad, _element: &super::FragMp4Pay, event: gst::Event) -> bool {
        gst_log!(CAT, obj: pad, "Handling event {:?}", event);
        self.sinkpad.push_event(event)
    }

    fn src_query(
        &self,
        pad: &gst::Pad,
        _element: &super::FragMp4Pay,
        query: &mut gst::QueryRef,
    ) -> bool {
        gst_log!(CAT, obj: pad, "Handling query {:?}", query);
        self.sinkpad.peer_query(query)
    }
}

#[glib::object_subclass]
impl ObjectSubclass for FragMp4Pay {
    const NAME: &'static str = ELEMENT_CLASS_NAME;
    type Type = super::FragMp4Pay;
    type ParentType = gst::Element;

    fn with_class(klass: &Self::Class) -> Self {
        let templ = klass.get_pad_template("sink").unwrap();
        let sinkpad = gst::Pad::builder_with_template(&templ, Some("sink"))
            .chain_function(|pad, parent, buffer| {
                FragMp4Pay::catch_panic_pad_function(
                    parent,
                    || Err(gst::FlowError::Error),
                    |identity, element| identity.sink_chain(pad, element, buffer),
                )
            })
            .event_function(|pad, parent, event| {
                FragMp4Pay::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.sink_event(pad, element, event),
                )
            })
            .query_function(|pad, parent, query| {
                FragMp4Pay::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.sink_query(pad, element, query),
                )
            })
            .build();

        let templ = klass.get_pad_template("src").unwrap();
        let srcpad = gst::Pad::builder_with_template(&templ, Some("src"))
            .event_function(|pad, parent, event| {
                FragMp4Pay::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.src_event(pad, element, event),
                )
            })
            .query_function(|pad, parent, query| {
                FragMp4Pay::catch_panic_pad_function(
                    parent,
                    || false,
                    |identity, element| identity.src_query(pad, element, query),
                )
            })
            .build();

        Self {
            state: Mutex::new(Default::default()),
            srcpad,
            sinkpad,
        }
    }
}

impl ObjectImpl for FragMp4Pay {
    fn constructed(&self, obj: &Self::Type) {
        self.parent_constructed(obj);
        obj.add_pad(&self.sinkpad).unwrap();
        obj.add_pad(&self.srcpad).unwrap();
    }
}

impl ElementImpl for FragMp4Pay {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: Lazy<gst::subclass::ElementMetadata> = Lazy::new(|| {
            gst::subclass::ElementMetadata::new(
                ELEMENT_LONG_NAME,
                "Generic",
                "TODO description\n
                ",
                "Claudio Fahey <claudio.fahey@dell.com>",
                )
        });
        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: Lazy<Vec<gst::PadTemplate>> = Lazy::new(|| {
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

    fn change_state(
        &self,
        element: &Self::Type,
        transition: gst::StateChange,
    ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
        gst_debug!(CAT, obj: element, "Changing state {:?}", transition);
        self.parent_change_state(element, transition)
    }
}