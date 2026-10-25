"""WebView2 原生宿主生命周期，初始化失败不能依赖网页回报。"""
import logging
import json

from flyout_material import prepare_surface
from flyout_native import apply_acrylic, position_window, run_on_ui, window_handle
from flyout_scripts import FlyoutScripts

logger = logging.getLogger(__name__)


class FlyoutHost:
    """负责 UI 线程上的原生显隐、首屏准备与 WebView2 失败事件。"""

    def __init__(self, window, context):
        self.window = window
        self._context = context
        self.hwnd = None
        self.scripts = FlyoutScripts(window, context['error'])
        self._preferences_registered = False
        self._disposed = False
        self._material_pending = False
        self._dark = False
        self._memory_api_warned = False
        window.events.before_show += self._prepare
        window.events.loaded += self._check_frontend
        window.events.closed += self.scripts.close

    def _prepare(self):
        """首次不可见显示前准备原生底面，且不同步调用 JavaScript。"""
        try:
            self.hwnd = window_handle(self.window)
            # 仅替换本窗口实例的桥接派发，避免 pywebview API 回包仍走
            # 无期限 Semaphore.acquire；不改安装目录或全局框架实现。
            self.window.native.browser.evaluate_js = self._evaluate_bridge_script
            view = self.window.native.webview
            view.CoreWebView2InitializationCompleted += self._on_initialized
            view.NavigationCompleted += self._on_navigated
            from Microsoft.Win32 import SystemEvents
            SystemEvents.UserPreferenceChanged += self._on_preferences_changed
            self._preferences_registered = True
            self.window.native.FormClosed += self._dispose_on_ui
            prepare_surface(self.window)
            position_window(self.window, self._context['mode'](), self._context['anchor']())
        except Exception as error:
            # pywebview 会捕获事件异常，因此在这里显式进入统一失败退出路径。
            logger.exception('原生窗口准备失败')
            self._context['error'](str(error))

    def _on_initialized(self, sender, args):
        """WebView2 初始化失败时，原生侧关闭窗口并报告实际异常。"""
        if not args.IsSuccess:
            self._context['error'](f'WebView2 初始化失败：{args.InitializationException}')
            return
        try:
            prepare_surface(self.window)
            sender.CoreWebView2.ProcessFailed += self._on_process_failed
            logger.info('GUI WebView2 Runtime: %s',
                        sender.CoreWebView2.Environment.BrowserVersionString)
        except Exception as error:
            self._context['error'](str(error))

    def _evaluate_bridge_script(self, script, parse_json):
        """保留 pywebview 双层 JSON 协议，同时为普通 API 回包提供超时保护。"""
        result = self.scripts.evaluate(script)
        if parse_json and isinstance(result, str):
            try:
                return json.loads(result)
            except (ValueError, TypeError):
                return result
        return result

    def _on_process_failed(self, sender, args):
        """渲染进程故障进入显式关闭路径，而非保留一扇永久无响应的窗口。"""
        kind = getattr(args, 'ProcessFailedKind', 'Unknown')
        details = [f'kind={kind}']
        reason = getattr(args, 'Reason', None)
        if reason is not None:
            details.append(f'reason={reason}')
        exit_code = getattr(args, 'ExitCode', None)
        if exit_code is not None:
            if isinstance(exit_code, int):
                details.append(f'exit_code=0x{exit_code & 0xFFFFFFFF:08X}')
            else:
                details.append(f'exit_code={exit_code}')
        logger.error('WebView2 进程故障事件: %s', ', '.join(details))
        self._context['error'](f'WebView2 进程故障: {kind}')

    def _on_preferences_changed(self, sender, args):
        """框架会在系统偏好通知时覆盖 DWM 38；排队到通知之后恢复 Acrylic。"""
        from System import Action
        if self._disposed or self._material_pending:
            return
        self._material_pending = True
        try:
            self.window.native.BeginInvoke(Action(self._restore_material))
        except Exception:
            self._material_pending = False
            if not self._disposed:
                logger.exception('系统背景材质恢复派发失败')

    def _restore_material(self):
        self._material_pending = False
        if not self._disposed:
            try:
                dark = self._context.get('dark', lambda: self._dark)()
                self.refresh_appearance(dark)
            except Exception as error:
                self._context['error'](str(error))

    def _dispose_on_ui(self, sender, args):
        """静态系统事件必须解绑，避免窗口关闭后继续回调已释放的句柄。"""
        self._disposed = True
        self.scripts.close()
        if self._preferences_registered:
            from Microsoft.Win32 import SystemEvents
            SystemEvents.UserPreferenceChanged -= self._on_preferences_changed
            self._preferences_registered = False

    def refresh_appearance(self, dark):
        self._dark = bool(dark)
        apply_acrylic(self.window, self._dark)

    def _on_navigated(self, sender, args):
        """导航失败没有前端可回报，必须由宿主报告错误码。"""
        if not args.IsSuccess:
            self._context['error'](f'GUI 页面加载失败：{args.WebErrorStatus}')

    def _check_frontend(self):
        """loaded 回调运行在后台线程，可读取桥接就绪前捕获的脚本错误。"""
        try:
            failure = self.scripts.evaluate('window.__getFrontendFailure()')
            if failure:
                self._context['error'](failure)
        except Exception as error:
            # 启动脚本缺失或桥接读取失败同样不能留下永久隐藏的窗口。
            if self.scripts.closed:
                return
            logger.exception('GUI 启动脚本检查失败')
            self._context['error'](str(error))

    def present(self, options):
        """定位、材质和可见状态作为一次 UI 操作完成，不触碰页面样式。"""
        def show():
            position_window(self.window, options['mode'], options['anchor'])
            self.refresh_appearance(options['dark'])
            self.window.native.Show()
            self.window.native.Activate()
        run_on_ui(self.window, show)

    def hide(self):
        """先隐藏，再进入官方低内存模式；短期保留可立即恢复的页面。"""
        run_on_ui(self.window, self.window.native.Hide)
        self.set_background_mode(True)

    def set_background_mode(self, background):
        """不暂停脚本，避免 API 回包超时；不与 TrySuspend/Resume 混用。"""
        def apply():
            if self._disposed:
                return
            core = self.window.native.webview.CoreWebView2
            if core is None:
                return
            try:
                from Microsoft.Web.WebView2.Core import CoreWebView2MemoryUsageTargetLevel
                core.MemoryUsageTargetLevel = (CoreWebView2MemoryUsageTargetLevel.Low if background
                                               else CoreWebView2MemoryUsageTargetLevel.Normal)
            except Exception:
                # 旧 SDK/运行时不支持此优化时仍可显隐，并由闲置退出释放内存。
                if not self._memory_api_warned:
                    logger.warning('当前 WebView2 无法设置低内存目标，保留闲置退出策略', exc_info=True)
                    self._memory_api_warned = True
        run_on_ui(self.window, apply)
