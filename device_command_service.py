"""设备命令执行与 GUI 请求回执；硬件访问仍限定在托盘进程。"""
import logging
import threading

logger = logging.getLogger(__name__)
# 重新枚举硬件并读取电量，与仅同步共享快照区分。
DEVICE_COMMAND_SCAN_DEVICES = 'scan_devices'
# 有完成回执的 GUI 命令；键值同时约束可调用的硬件方法。
COMMAND_METHODS = {
    'scan_devices': 'scan_and_refresh',
    'scan_keyboard_candidates': '_scan_keyboard_candidates',
    'bind_keyboard': '_bind_keyboard',
    'unbind_keyboard': '_unbind_keyboard',
    'scan_bluetooth_candidates': '_scan_bluetooth_candidates',
    'bind_bluetooth': '_bind_bluetooth',
    'unbind_bluetooth': '_unbind_bluetooth',
    'refresh_battery': 'manual_refresh',
}
# 需要设备 ID 的动作，不允许空 ID 被当作已执行。
DEVICE_ID_ACTIONS = frozenset(('bind_keyboard', 'bind_bluetooth', 'unbind_bluetooth'))
# 支持旧硬件方法的请求标识参数。
REQUEST_ID_ACTIONS = frozenset(('bind_keyboard', 'bind_bluetooth', 'scan_bluetooth_candidates', 'refresh_battery'))
# 与托盘命令监听周期一致的共享快照读取间隔。
COMMAND_POLL_SECONDS = 0.15


def execute_reported_command(manager, command):
    """在托盘进程执行命令，并发布按请求 ID 区分的最终回执。"""
    action, request_id = command['action'], command['request_id']
    record = {'request_id': request_id, 'state': 'running', 'message': ''}
    manager.record_command_result(action, record)
    try:
        invoke_device_method(manager, command)
        message = command_failure(manager, action)
        if message:
            raise RuntimeError(message)
        record = {'request_id': request_id, 'state': 'completed', 'message': ''}
    except Exception as error:
        # 硬件失败保留明确错误回执，GUI 不把空列表或旧快照伪装成成功。
        logger.exception('GUI 设备命令失败: %s', action)
        record = {'request_id': request_id, 'state': 'error', 'message': str(error)}
    manager.record_command_result(action, record)


def invoke_device_method(manager, command):
    """仅分发已登记动作，设备 ID 在跨进程边界校验。"""
    action = command['action']
    method = getattr(manager, COMMAND_METHODS[action])
    args = []
    if action in DEVICE_ID_ACTIONS:
        device_id = command.get('payload', {}).get('device_id')
        if not isinstance(device_id, str) or not device_id.strip():
            raise ValueError('设备 ID 不能为空')
        args.append(device_id)
    if action in REQUEST_ID_ACTIONS:
        args.append(command['request_id'])
    method(*args)


def command_failure(manager, action):
    """读取原硬件方法已发布的失败状态，避免吞掉其内部异常。"""
    if action == 'bind_keyboard':
        _, state, message = manager.keyboard_binding_result
        return message if state == 'error' else ''
    if action == 'scan_keyboard_candidates':
        return manager.keyboard_scan_message if manager.keyboard_scan_state == 'error' else ''
    if action in ('bind_bluetooth', 'scan_bluetooth_candidates'):
        return manager.bluetooth_scan_message if manager.bluetooth_scan_state == 'error' else ''
    if action == 'refresh_battery':
        return manager.battery_refresh_message if manager.battery_refresh_state == 'error' else ''
    return ''


class DeviceCommandClient:
    """GUI 端串行提交命令，等待对应回执；关闭窗口不取消托盘侧绑定。"""

    def __init__(self, shared_manager, options):
        self._shared = shared_manager
        self._send = options['send']
        self._host_alive = options['host_alive']
        self._closed = options['closed']
        self._on_progress = options['on_progress']
        self._lock = threading.Lock()

    def execute(self, action, payload=None):
        """同一 GUI 按顺序发送操作，绝不复用旧请求的成功状态。"""
        with self._lock:
            if not self._host_alive():
                raise RuntimeError('托盘主进程未运行，请从托盘打开窗口')
            request_id = self._send(action, {**(payload or {}), 'report_result': True})
            while not self._closed.is_set():
                # 只读取回执；有变化的状态由注入的推送器发布，避免全应用重复刷新。
                self._shared.sync_shared_state_silently()
                self._on_progress()
                if self._shared.last_read_state == 'error':
                    raise RuntimeError(self._shared.last_read_error)
                result = self._shared.command_results.get(action, {})
                if result.get('request_id') == request_id and result.get('state') != 'running':
                    if result.get('state') == 'error':
                        raise RuntimeError(result.get('message'))
                    return result
                if result.get('request_id', 0) > request_id:
                    raise RuntimeError('设备操作已被另一个窗口的请求替代')
                if not self._host_alive():
                    raise RuntimeError('等待设备操作时托盘主进程退出')
                self._closed.wait(COMMAND_POLL_SECONDS)
            raise RuntimeError('窗口已关闭；托盘中的设备操作继续执行')
