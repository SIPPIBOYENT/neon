from fixtures.neon_fixtures import NeonEnvBuilder
from uuid import UUID, uuid4
import tarfile
import os
import shutil
from pathlib import Path
import json


def test_import_from_vanilla(test_output_dir, pg_bin, vanilla_pg, neon_env_builder):

    # HACK
    basebackup_cache = "/home/bojan/tmp/basebackup"
    # basebackup_cache = None

    basebackup_dir = os.path.join(test_output_dir, "basebackup")
    if basebackup_cache:
        basebackup_dir = basebackup_cache
    else:
        vanilla_pg.start()
        vanilla_pg.safe_psql("create table t as select generate_series(1,300000)")
        assert vanilla_pg.safe_psql('select count(*) from t') == [(300000, )]

        vanilla_pg.safe_psql("CHECKPOINT")
        os.mkdir(basebackup_dir)
        pg_bin.run([
            "pg_basebackup",
            "-F", "tar",
            "-d", vanilla_pg.connstr(),
            "-D", basebackup_dir,
        ])

    with open(os.path.join(basebackup_dir, "backup_manifest")) as f:
        manifest = json.load(f)
        start_lsn = manifest["WAL-Ranges"][0]["Start-LSN"]
        end_lsn = manifest["WAL-Ranges"][0]["End-LSN"]

    node_name = "import_from_vanilla"
    tenant = uuid4()
    timeline = uuid4()

    env = neon_env_builder.init_start()
    env.neon_cli.create_tenant(tenant)
    env.neon_cli.raw_cli([
        "timeline",
        "import",
        "--tenant-id", tenant.hex,
        "--timeline-id", timeline.hex,
        "--node-name", node_name,
        # created manually with: tar -cvf basebackup.tar <all files, with pg_wal at the end>
        "--tarfile", os.path.join(basebackup_dir, "basebackup.tar"),
        "--lsn", end_lsn,
        # "--tarfile", os.path.join(basebackup_dir, "base.tar"),
        # "--lsn", start_lsn,
    ])
    pg = env.postgres.create_start(node_name, tenant_id=tenant)
    assert pg.safe_psql('select count(*) from t') == [(300000, )]


def test_import_from_neon(neon_env_builder,
                          port_distributor,
                          default_broker,
                          mock_s3_server,
                          test_output_dir,
                          pg_bin):
    """Move a timeline to a new neon stack using pg_basebackup as interface."""
    node_name = "test_import"
    source_repo_dir = Path(test_output_dir) / "source_repo"
    destination_repo_dir = Path(test_output_dir) / "destination_repo"
    basebackup_dir = Path(test_output_dir) / "basebackup"
    basebackup_tar_path = Path(test_output_dir) / "basebackup.tar"
    os.mkdir(basebackup_dir)

    # Create a repo, put some data in, take basebackup, and shut it down
    with NeonEnvBuilder(source_repo_dir, port_distributor, default_broker, mock_s3_server) as builder:

        # Insert data
        env = builder.init_start()
        env.neon_cli.create_branch(node_name)
        pg = env.postgres.create_start(node_name)
        pg.safe_psql("create table t as select generate_series(1,300000)")
        assert pg.safe_psql('select count(*) from t') == [(300000, )]

        # Get basebackup
        lsn = pg.safe_psql('select pg_current_wal_flush_lsn()')[0][0]
        tenant = pg.safe_psql("show neon.tenant_id")[0][0]
        timeline = pg.safe_psql("show neon.timeline_id")[0][0]
        timeline_dir = source_repo_dir / "tenants" / tenant / "timelines" / timeline
        pg_bin.run(["pg_basebackup", "-d", pg.connstr(), "-D", str(basebackup_dir)])

        # Pack basebackup into tar file (uncompressed)
        with tarfile.open(basebackup_tar_path, "w") as tf:
            # TODO match iteration order to what pageserver would do
            tf.add(basebackup_dir)

        # Remove timeline
        # env.pageserver.stop()
        # shutil.rmtree(timeline_dir)
        env.pageserver.http_client().timeline_detach(UUID(tenant), UUID(timeline))

        # env.neon_cli.create_tenant(UUID(tenant))
        env.neon_cli.raw_cli([
            "timeline",
            "import",
            "--tenant-id", tenant,
            "--timeline-id", timeline,
            "--node-name", node_name,
            "--tarfile", str(basebackup_tar_path),
            "--lsn", lsn,
        ])

        # pg.stop_and_destroy()
        # pg = env.postgres.create_start(node_name, tenant_id=UUID(tenant))

        assert pg.safe_psql('select count(*) from t') == [(300000, )]


    # XXX
    return


    # Create a new repo, load the basebackup into it, and check that data is there
    with NeonEnvBuilder(destination_repo_dir, port_distributor, default_broker, mock_s3_server) as builder:
        env = builder.init_start()
        env.neon_cli.create_tenant(UUID(tenant))
        env.neon_cli.raw_cli([
            "timeline",
            "import",
            "--tenant-id", tenant,
            "--timeline-id", timeline,
            "--node-name", node_name,
            "--tarfile", basebackup_tar_path,
        ])
        pg = env.postgres.create_start(node_name, tenant_id=UUID(tenant))
        assert pg.safe_psql('select count(*) from t') == [(300000, )]
