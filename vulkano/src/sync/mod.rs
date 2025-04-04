//! Synchronization on the GPU.
//!
//! Just like for CPU code, you have to ensure that buffers and images are not accessed mutably by
//! multiple GPU queues simultaneously and that they are not accessed mutably by the CPU and by the
//! GPU simultaneously.
//!
//! This safety is enforced at runtime by vulkano but it is not magic and you will require some
//! knowledge if you want to avoid errors.

#[allow(unused)]
pub(crate) use self::pipeline::*;
pub use self::{
    future::{now, GpuFuture},
    pipeline::{
        AccessFlags, BufferMemoryBarrier, DependencyFlags, DependencyInfo, ImageMemoryBarrier,
        MemoryBarrier, PipelineStage, PipelineStages, QueueFamilyOwnershipTransfer,
    },
};
use crate::{device::Queue, VulkanError};
use ash::vk;
use smallvec::SmallVec;
use std::{
    error::Error,
    fmt::{Display, Formatter},
    sync::Arc,
};

pub mod event;
pub mod fence;
pub mod future;
mod pipeline;
pub mod semaphore;

/// Declares in which queue(s) a resource can be used.
///
/// When you create a buffer or an image, you have to tell the Vulkan library in which queue
/// families it will be used. The vulkano library requires you to tell in which queue family
/// the resource will be used, even for exclusive mode.
#[derive(Debug, Clone, PartialEq, Eq)]
// TODO: remove
pub enum SharingMode {
    /// The resource is used is only one queue family.
    Exclusive,
    /// The resource is used in multiple queue families. Can be slower than `Exclusive`.
    Concurrent(Vec<u32>), // TODO: Vec is too expensive here
}

impl<'a> From<&'a Arc<Queue>> for SharingMode {
    #[inline]
    fn from(_queue: &'a Arc<Queue>) -> SharingMode {
        SharingMode::Exclusive
    }
}

impl<'a> From<&'a [&'a Arc<Queue>]> for SharingMode {
    #[inline]
    fn from(queues: &'a [&'a Arc<Queue>]) -> SharingMode {
        SharingMode::Concurrent(
            queues
                .iter()
                .map(|queue| queue.queue_family_index())
                .collect(),
        )
    }
}

/// Declares in which queue(s) a resource can be used.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Sharing<I>
where
    I: IntoIterator<Item = u32>,
{
    /// The resource is used is only one queue family.
    Exclusive,
    /// The resource is used in multiple queue families. Can be slower than `Exclusive`.
    Concurrent(I),
}

impl Sharing<SmallVec<[u32; 4]>> {
    /// Returns `true` if `self` is the `Exclusive` variant.
    #[inline]
    pub fn is_exclusive(&self) -> bool {
        matches!(self, Self::Exclusive)
    }

    /// Returns `true` if `self` is the `Concurrent` variant.
    #[inline]
    pub fn is_concurrent(&self) -> bool {
        matches!(self, Self::Concurrent(..))
    }

    pub(crate) fn to_vk(&self) -> (vk::SharingMode, &[u32]) {
        match self {
            Sharing::Exclusive => (vk::SharingMode::EXCLUSIVE, [].as_slice()),
            Sharing::Concurrent(queue_family_indices) => {
                (vk::SharingMode::CONCURRENT, queue_family_indices.as_slice())
            }
        }
    }
}

/// How the memory of a resource is currently being accessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CurrentAccess {
    /// The resource is currently being accessed exclusively by the CPU.
    CpuExclusive,

    /// The resource is currently being accessed exclusively by the GPU.
    /// The GPU can have multiple exclusive accesses, if they are separated by synchronization.
    ///
    /// `gpu_writes` must not be 0. If it's decremented to 0, switch to `Shared`.
    GpuExclusive { gpu_reads: usize, gpu_writes: usize },

    /// The resource is not currently being accessed, or is being accessed for reading only.
    Shared { cpu_reads: usize, gpu_reads: usize },
}

/// Error when attempting to read or write a resource from the host (CPU).
#[derive(Clone, Debug)]
pub enum HostAccessError {
    AccessConflict(AccessConflict),
    Invalidate(VulkanError),
    Unmanaged,
    NotHostMapped,
    OutOfMappedRange,
}

impl Error for HostAccessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::AccessConflict(err) => Some(err),
            Self::Invalidate(err) => Some(err),
            _ => None,
        }
    }
}

impl Display for HostAccessError {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            Self::AccessConflict(_) => {
                write!(f, "the resource is already in use in a conflicting way")
            }
            Self::Unmanaged => write!(f, "the resource is not managed by vulkano"),
            HostAccessError::Invalidate(_) => write!(f, "invalidating the device memory failed"),
            HostAccessError::NotHostMapped => {
                write!(f, "the device memory is not current host-mapped")
            }
            HostAccessError::OutOfMappedRange => write!(
                f,
                "the requested range is not within the currently mapped range of device memory",
            ),
        }
    }
}

/// Conflict when attempting to access a resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessConflict {
    /// The resource is already locked for reading by the host (CPU).
    HostRead,

    /// The resource is already locked for writing by the host (CPU).
    HostWrite,

    /// The resource is already locked for reading by the device (GPU).
    DeviceRead,

    /// The resource is already locked for writing by the device (GPU).
    DeviceWrite,
}

impl Error for AccessConflict {}

impl Display for AccessConflict {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        match self {
            AccessConflict::HostRead => write!(
                f,
                "the resource is already locked for reading by the host (CPU)"
            ),
            AccessConflict::HostWrite => write!(
                f,
                "the resource is already locked for writing by the host (CPU)"
            ),
            AccessConflict::DeviceRead => write!(
                f,
                "the resource is already locked for reading by the device (GPU)"
            ),
            AccessConflict::DeviceWrite => write!(
                f,
                "the resource is already locked for writing by the device (GPU)"
            ),
        }
    }
}
