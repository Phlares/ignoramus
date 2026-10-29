//! One experiment row in a virtual machine: the host side, the guest agent, and the control
//! frames, output pack and output hash that both sides use.
#![deny(unsafe_code)]
pub mod agent;
pub mod host;
pub mod image;
pub mod pack;
pub mod proto;
pub mod sys;
pub mod tree;
