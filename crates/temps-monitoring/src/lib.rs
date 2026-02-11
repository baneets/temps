//! Monitoring services and utilities
//!
//! This crate provides system monitoring capabilities including:
//! - Disk space monitoring with configurable thresholds
//! - Outage detection and notification for status monitors
//! - Future: CPU, memory, network monitoring

pub mod disk_space;
pub mod outage;
pub mod services;

pub use disk_space::*;
pub use outage::*;
pub use services::*;
