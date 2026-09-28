import tempfile
import unittest
from unittest import mock

import updater
import update_ipc


class _Asset:
    def __init__(self, version='2.6.2', notes='release notes'):
        self.Version = version
        self.NotesMarkdown = notes
        self.NotesHtml = ''


class _UpdateInfo:
    def __init__(self, version='2.6.2', notes='release notes'):
        self.TargetFullRelease = _Asset(version, notes)


class _Manager:
    def __init__(self, update_info=None):
        self.update_info = update_info
        self.downloaded = None
        self.progress_values = []
        self.applied = None

    def check_for_updates(self):
        return self.update_info

    def download_updates(self, update_info, callback):
        self.downloaded = update_info
        callback(24)
        callback(100)

    def get_update_pending_restart(self):
        return 'pending-update'

    def apply_updates_and_restart_with_args(self, update, args):
        self.applied = (update, args)


class UpdaterTests(unittest.TestCase):
    def test_initialize_runs_velopack_startup_hook_once(self):
        app = mock.Mock()
        app.set_auto_apply_on_startup.return_value = app
        with mock.patch.object(updater, 'App', return_value=app) as app_factory:
            updater.initialize_velopack()

        app_factory.assert_called_once_with()
        app.set_auto_apply_on_startup.assert_called_once_with(False)
        app.run.assert_called_once_with()

    def test_check_for_update_returns_velopack_candidate(self):
        update_info = _UpdateInfo()
        manager = _Manager(update_info)
        with mock.patch.object(updater, '_create_update_manager', return_value=manager):
            result = updater.check_for_update('2.6.1')

        self.assertTrue(result.has_update)
        self.assertEqual(result.latest_version, '2.6.2')
        self.assertEqual(result.release_notes, 'release notes')
        self.assertIsNotNone(result.candidate)
        self.assertIs(result.candidate.manager, manager)
        self.assertIs(result.candidate.update_info, update_info)

    def test_check_for_update_reports_explicit_error(self):
        manager = _Manager()
        manager.check_for_updates = mock.Mock(side_effect=RuntimeError('not installed'))
        with mock.patch.object(updater, '_create_update_manager', return_value=manager):
            result = updater.check_for_update('2.6.1')

        self.assertFalse(result.has_update)
        self.assertEqual(result.latest_version, '2.6.1')
        self.assertIn('RuntimeError: not installed', result.error)

    def test_source_mode_install_is_rejected_explicitly(self):
        candidate = updater.UpdateCandidate(_Manager(), object(), '2.6.2', 'notes')
        statuses = []
        with mock.patch.object(updater.sys, 'frozen', False, create=True):
            success = updater.download_and_install(
                candidate,
                on_status=lambda stage, detail='': statuses.append((stage, detail)),
            )

        self.assertFalse(success)
        self.assertEqual(statuses[-1][0], 'error')
        self.assertIn('源码模式', statuses[-1][1])

    def test_main_process_downloads_and_applies_pending_update(self):
        manager = _Manager()
        candidate = updater.UpdateCandidate(manager, object(), '2.6.2', 'notes')
        progress = []
        statuses = []
        with (
            mock.patch.object(updater.sys, 'frozen', True, create=True),
            mock.patch.object(updater, 'request_process_shutdown', return_value=True),
            mock.patch.object(updater, 'wait_for_shutdown_ack', return_value={'status': 'accepted'}),
        ):
            success = updater.download_and_install(
                candidate,
                on_progress=progress.append,
                on_status=lambda stage, detail='': statuses.append((stage, detail)),
            )

        self.assertTrue(success)
        self.assertEqual(progress, [24, 100])
        self.assertIsNotNone(manager.downloaded)
        self.assertIsNone(manager.applied)
        self.assertEqual([stage for stage, _ in statuses], ['downloading', 'verifying', 'applying'])

    def test_gui_process_downloads_then_notifies_host_process(self):
        manager = _Manager()
        candidate = updater.UpdateCandidate(manager, object(), '2.6.2', 'notes')
        with (
            mock.patch.object(updater.sys, 'frozen', True, create=True),
            mock.patch.object(updater.os, 'getpid', return_value=456),
            mock.patch.object(updater, 'request_process_shutdown', return_value=True) as shutdown,
            mock.patch.object(updater, 'wait_for_shutdown_ack', return_value={'status': 'accepted'}),
        ):
            success = updater.download_and_install(candidate, host_pid=123)

        self.assertTrue(success)
        self.assertIsNone(manager.applied)
        shutdown.assert_called_once_with(target_pid=123, reason='update', skip_gui_pid=456)

    def test_missing_candidate_is_rejected(self):
        statuses = []
        success = updater.download_and_install(
            None,
            on_status=lambda stage, detail='': statuses.append((stage, detail)),
        )

        self.assertFalse(success)
        self.assertEqual(statuses[-1][0], 'error')
        self.assertIn('更新候选', statuses[-1][1])

    def test_shutdown_request_round_trip(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            with mock.patch.object(update_ipc.tempfile, 'gettempdir', return_value=temp_dir):
                self.assertTrue(updater.request_process_shutdown(123, skip_gui_pid=456))
                payload = updater.consume_shutdown_request(123)

        self.assertEqual(payload['reason'], 'update')
        self.assertEqual(payload['target_pid'], 123)
        self.assertEqual(payload['skip_gui_pid'], 456)

    def test_shutdown_ack_round_trip(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            with mock.patch.object(update_ipc.tempfile, 'gettempdir', return_value=temp_dir):
                request = {'requester_pid': 456, 'target_pid': 123}
                self.assertTrue(updater.acknowledge_shutdown(request, 'accepted'))
                payload = updater.wait_for_shutdown_ack(456, 123, timeout_seconds=0.1)

        self.assertEqual(payload['status'], 'accepted')


if __name__ == '__main__':
    unittest.main()
