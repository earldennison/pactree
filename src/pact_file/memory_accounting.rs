use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
mod tests;

static CACHE_RESIDENT_BYTES: AtomicUsize = AtomicUsize::new(0);
static IN_FLIGHT_BYTES: AtomicUsize = AtomicUsize::new(0);
static IN_FLIGHT_PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn cache_resident_add(bytes: usize) {
    saturating_add(&CACHE_RESIDENT_BYTES, bytes);
}

pub(crate) fn cache_resident_sub(bytes: usize) {
    saturating_sub(&CACHE_RESIDENT_BYTES, bytes);
}

pub(crate) fn cache_resident_bytes() -> usize {
    CACHE_RESIDENT_BYTES.load(Ordering::Relaxed)
}

pub(crate) fn in_flight_bytes() -> usize {
    IN_FLIGHT_BYTES.load(Ordering::Relaxed)
}

pub(crate) fn in_flight_peak_bytes() -> usize {
    IN_FLIGHT_PEAK_BYTES.load(Ordering::Relaxed)
}

pub(crate) fn reset_in_flight_peak() {
    IN_FLIGHT_PEAK_BYTES.store(IN_FLIGHT_BYTES.load(Ordering::Relaxed), Ordering::Relaxed);
}

fn saturating_add(gauge: &AtomicUsize, bytes: usize) -> usize {
    let mut current = gauge.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_add(bytes);
        match gauge.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(observed) => current = observed,
        }
    }
}

fn saturating_sub(gauge: &AtomicUsize, bytes: usize) {
    let mut current = gauge.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_sub(bytes);
        match gauge.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

fn in_flight_add(bytes: usize) {
    let now = saturating_add(&IN_FLIGHT_BYTES, bytes);
    IN_FLIGHT_PEAK_BYTES.fetch_max(now, Ordering::Relaxed);
}

fn in_flight_sub(bytes: usize) {
    saturating_sub(&IN_FLIGHT_BYTES, bytes);
}

pub(crate) struct Budget {
    limit: usize,
    used: AtomicUsize,
    peak: AtomicUsize,
}

impl Budget {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    pub(crate) fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    pub(crate) fn try_reserve(&self, bytes: usize) -> Option<Lease<'_>> {
        let mut current = self.used.load(Ordering::Relaxed);
        loop {
            let next = current.checked_add(bytes)?;
            if next > self.limit {
                return None;
            }
            match self.used.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        self.peak.fetch_max(current + bytes, Ordering::Relaxed);
        in_flight_add(bytes);
        Some(Lease {
            budget: self,
            bytes,
        })
    }

    pub(crate) fn reserve_tracked(&self, bytes: usize) -> Lease<'_> {
        let now = saturating_add(&self.used, bytes);
        self.peak.fetch_max(now, Ordering::Relaxed);
        in_flight_add(bytes);
        Lease {
            budget: self,
            bytes,
        }
    }
}

pub(crate) struct Lease<'a> {
    budget: &'a Budget,
    bytes: usize,
}

impl Lease<'_> {
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        saturating_sub(&self.budget.used, self.bytes);
        in_flight_sub(self.bytes);
    }
}
