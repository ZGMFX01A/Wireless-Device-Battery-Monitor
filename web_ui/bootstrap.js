// 在模块和 Python 桥接初始化前捕获失败，原生 loaded 回调会读取这里的实际错误。
(() => {
    let failure = null;
    window.__getFrontendFailure = () => failure;
    window.__reportFrontendFailure = message => {
        failure ??= String(message);
        console.error('GUI frontend failed', failure);
        if (window.pywebview?.api?.on_frontend_error) {
            window.pywebview.api.on_frontend_error(failure).catch(error => console.error(error));
        }
    };
    window.addEventListener('error', event => {
        const resource = event.target?.src || event.target?.href;
        window.__reportFrontendFailure(event.error?.message || event.message || `GUI resource failed: ${resource}`);
    }, true);
    window.addEventListener('unhandledrejection', event => {
        window.__reportFrontendFailure(event.reason?.message || event.reason);
    });
})();
