"""独立 GUI 启动器；使用当前 WebView2 设置入口。"""
import sys
from flyout import launch_flyout_process

if __name__ == '__main__':
    sys.exit(launch_flyout_process(mode='settings'))
