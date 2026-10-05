//! G4 gate: steady-state upload worker preparation must not touch the heap.

use arty_core::{BlendMode, Document, TILE_SIZE, TileCoord, fix15};
use arty_render::gpu::UploadRect;
use arty_render::upload::{

    CHAIN_BYTES, RowJob, Worker, row_jobs, run_jobs_fresh, run_jobs_reused, run_jobs_seq,
    with_thread_worker,
};

#[global_allocator]
static ALLOC: arty_testkit::CountingAllocator = arty_testkit::CountingAllocator;

/// `peak_bytes_during` counts the whole process, so the tests of this binary
/// run one at a time (cargo runs them on parallel threads).
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn build_test_doc(tiles_x: u32, tiles_y: u32) -> Document {
    let mut doc = Document::new(tiles_x * TILE_SIZE as u32, tiles_y * TILE_SIZE as u32, 350);
    doc.set_paper(Some([fix15::ONE_U16; 4]));
    let base = doc.active();
    let folder = doc.add_folder().unwrap();
    let clip = doc.add_raster_layer().unwrap();
    doc.move_layer(clip, Some(folder), 0);
    let inner_base = doc.add_raster_layer().unwrap();
    doc.move_layer(inner_base, Some(folder), 0);

    let mut p = doc.layer(clip).unwrap().props.clone();
    p.clip = true;
    p.blend = BlendMode::Multiply;
    doc.set_props(clip, p);

    for (layer_idx, id) in [base, clip, inner_base].into_iter().enumerate() {
        let (grid, _) = doc.paint_target(id).unwrap();
        for y in 0..tiles_y {
            for x in 0..tiles_x {
                let v = ((layer_idx as u16 + 1) * 3000 + (x * 7 + y * 13) as u16 * 100) % 30000;
                let c = TileCoord::new(x as i32, y as i32);
                grid.get_mut_or_create(c).as_flattened_mut().fill([v / 2, v / 3, v / 4, v]);
            }
        }
    }
    doc
}

fn make_jobs<'a>(r: &UploadRect, buf: &'a mut [u8]) -> Vec<RowJob<'a>> {
    let mut jobs = Vec::new();
    row_jobs(r, buf, &mut jobs);
    jobs
}

#[test]
fn worker_run_is_allocation_free_when_warm() {
    let _serial = serial();
    let doc = build_test_doc(16, 1);
    let r = UploadRect { layer: 0, x: 0, y: 0, w: 16, h: 1 };
    let mut buf_warm = vec![0u8; 16 * CHAIN_BYTES];
    let mut buf_test = vec![0u8; 16 * CHAIN_BYTES];

    let mut worker = Worker::new();
    // Warm-up sizes the scratch stack and mip buffers.
    let jobs_warm = make_jobs(&r, &mut buf_warm);
    for job in jobs_warm {
        worker.run(&doc, job);
    }

    let jobs_test = make_jobs(&r, &mut buf_test);
    let n = arty_testkit::count_allocs(|| {
        for job in jobs_test {
            worker.run(&doc, job);
        }
    });
    assert_eq!(n, 0, "warm worker.run allocated {n} times");
    assert_eq!(buf_warm, buf_test);
}

#[test]
fn thread_local_worker_is_allocation_free_when_warm() {
    let _serial = serial();
    let doc = build_test_doc(16, 2);
    let r = UploadRect { layer: 0, x: 0, y: 0, w: 16, h: 2 };
    let mut buf_warm = vec![0u8; 32 * CHAIN_BYTES];
    let mut buf_test = vec![0u8; 32 * CHAIN_BYTES];

    // Warm-up on calling thread.
    let jobs_warm = make_jobs(&r, &mut buf_warm);
    with_thread_worker(|w| {
        for job in jobs_warm {
            w.run(&doc, job);
        }
    });

    let jobs_test = make_jobs(&r, &mut buf_test);
    let n = arty_testkit::count_allocs(|| {
        with_thread_worker(|w| {
            for job in jobs_test {
                w.run(&doc, job);
            }
        });
    });
    assert_eq!(n, 0, "warm thread-local worker allocated {n} times");
    assert_eq!(buf_warm, buf_test);
}

#[test]
fn run_jobs_seq_is_allocation_free_when_warm() {
    let _serial = serial();
    let doc = build_test_doc(16, 4);
    let r = UploadRect { layer: 0, x: 0, y: 0, w: 16, h: 4 };
    let mut buf_warm = vec![0u8; 64 * CHAIN_BYTES];
    let mut buf_test = vec![0u8; 64 * CHAIN_BYTES];

    let jobs_warm = make_jobs(&r, &mut buf_warm);
    run_jobs_seq(&doc, jobs_warm);

    let jobs_test = make_jobs(&r, &mut buf_test);
    let n = arty_testkit::count_allocs(|| {
        run_jobs_seq(&doc, jobs_test);
    });
    assert_eq!(n, 0, "run_jobs_seq allocated {n} times");
    assert_eq!(buf_warm, buf_test);
}

#[test]
fn parallel_worker_in_single_thread_pool_is_allocation_free_when_warm() {
    let _serial = serial();
    let doc = build_test_doc(16, 16);
    let r = UploadRect { layer: 0, x: 0, y: 0, w: 16, h: 16 };
    let mut buf_warm = vec![0u8; 256 * CHAIN_BYTES];
    let mut buf_test = vec![0u8; 256 * CHAIN_BYTES];

    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    pool.install(|| {
        let jobs_warm = make_jobs(&r, &mut buf_warm);
        run_jobs_reused(&doc, jobs_warm);

        let jobs_test = make_jobs(&r, &mut buf_test);
        let n = arty_testkit::count_allocs(|| {
            run_jobs_reused(&doc, jobs_test);
        });
        assert_eq!(n, 0, "parallel worker in 1-thread pool allocated {n} times");
    });
    assert_eq!(buf_warm, buf_test);
}

#[test]
fn multithreaded_upload_prep_steady_state_heap_growth_is_zero() {
    let _serial = serial();
    let doc = build_test_doc(16, 16);
    let r = UploadRect { layer: 0, x: 0, y: 0, w: 16, h: 16 };
    let mut buf_warm = vec![0u8; 256 * CHAIN_BYTES];
    let mut buf_test = vec![0u8; 256 * CHAIN_BYTES];

    // Warm-up every thread in Rayon's global pool.
    rayon::broadcast(|_| {
        let r_dummy = UploadRect { layer: 0, x: 0, y: 0, w: 1, h: 1 };
        let mut buf_dummy = vec![0u8; CHAIN_BYTES];
        let jobs_dummy = make_jobs(&r_dummy, &mut buf_dummy);
        with_thread_worker(|w| {
            for job in jobs_dummy {
                w.run(&doc, job);
            }
        });
    });

    // Steady-state measurement across all threads in the process.
    let jobs_test = make_jobs(&r, &mut buf_test);
    let (_, peak) = arty_testkit::peak_bytes_during(|| {
        run_jobs_reused(&doc, jobs_test);
    });
    assert_eq!(peak, 0, "warm upload prep caused {peak} B peak heap growth across threads");

    let jobs_warm = make_jobs(&r, &mut buf_warm);
    run_jobs_reused(&doc, jobs_warm);
    assert_eq!(buf_warm, buf_test);
}


#[test]
fn pre_e5_fresh_worker_allocates_heap() {
    let _serial = serial();
    // Sanity check proving that pre-E5 fresh worker path indeed allocates,
    // ensuring the CountingAllocator is active and tests are sensitive.
    let doc = build_test_doc(4, 1);
    let r = UploadRect { layer: 0, x: 0, y: 0, w: 4, h: 1 };
    let mut buf = vec![0u8; 4 * CHAIN_BYTES];

    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    pool.install(|| {
        let jobs = make_jobs(&r, &mut buf);
        let n = arty_testkit::count_allocs(|| {
            run_jobs_fresh(&doc, jobs);
        });
        assert!(n > 0, "run_jobs_fresh must allocate (pre-E5 baseline)");
    });
}
