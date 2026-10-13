"""在目标物理像素网格直接绘制托盘图标，避免二次缩放模糊。"""
import ctypes
from ctypes import wintypes
from functools import lru_cache
import os
from pathlib import Path
from types import MappingProxyType

from PIL import Image, ImageDraw, ImageFont
from config import TRAY_ICON_STYLE_CAPSULE

# 每种任务栏网格单独指定字体和电池轮廓，不缩小同一张 64px 图。
PIXEL_GRIDS = MappingProxyType({
    16: (13, 9, 10, 8, (0, 3, 13, 13), (3, 2, 12, 15)),
    24: (20, 13, 15, 12, (1, 5, 20, 19), (5, 3, 18, 23)),
    32: (26, 18, 21, 16, (1, 7, 27, 25), (6, 4, 25, 31)),
})
# 更高 DPI 也在实际网格直接绘制；96 DPI 的通知区为 16px。
BASE_ICON_SIZE = 16
BASE_SCALE_PERCENT = 100
# 字体来自 Windows 自带资源，路径不依赖进程工作目录。
FONT_NAME = 'segoeuib.ttf'
# 图标状态配色，在浅色任务栏使用更深的数字提高对比度。
COLORS = MappingProxyType({'charging': (6, 182, 212), 'good': (16, 185, 129),
                          'warning': (245, 158, 11), 'low': (239, 68, 68), 'unknown': (128, 128, 128)})


def taskbar_icon_size():
    """按主任务栏显示器缩放选择物理网格，不依赖 Python 的 DPI 感知状态。"""
    user = ctypes.WinDLL('user32', use_last_error=True)
    user.MonitorFromPoint.argtypes = [wintypes.POINT, wintypes.DWORD]
    user.MonitorFromPoint.restype = wintypes.HMONITOR
    monitor = user.MonitorFromPoint(wintypes.POINT(0, 0), 2)
    shcore = ctypes.WinDLL('shcore', use_last_error=True)
    shcore.GetScaleFactorForMonitor.argtypes = [wintypes.HMONITOR, ctypes.POINTER(ctypes.c_int)]
    shcore.GetScaleFactorForMonitor.restype = ctypes.c_long
    scale = ctypes.c_int()
    result = shcore.GetScaleFactorForMonitor(monitor, ctypes.byref(scale))
    if result != 0:
        raise OSError(f'无法读取任务栏缩放：HRESULT={result:#x}')
    return round(BASE_ICON_SIZE * scale.value / BASE_SCALE_PERCENT)


@lru_cache(maxsize=None)
def font(size):
    """复用同一字号字体，Windows 字体缺失时明确失败。"""
    path = Path(os.environ['WINDIR']) / 'Fonts' / FONT_NAME
    return ImageFont.truetype(str(path), size)


def pixel_grid(size):
    """标准网格用专项参数，其他 DPI 在实际尺寸上计算轮廓。"""
    if size in PIXEL_GRIDS:
        return PIXEL_GRIDS[size]
    ratio = size / BASE_ICON_SIZE
    values = PIXEL_GRIDS[BASE_ICON_SIZE]
    return tuple(round(value * ratio) if isinstance(value, int)
                 else tuple(round(coordinate * ratio) for coordinate in value) for value in values)


def battery_color(percentage, charging):
    """按业务状态着色，未知读数不归入低电量警告。"""
    if percentage < 0:
        return COLORS['unknown']
    if charging:
        return COLORS['charging']
    if percentage >= 50:
        return COLORS['good']
    return COLORS['warning'] if percentage >= 20 else COLORS['low']


def draw_text(draw, text, options):
    """在实际网格上居中绘字，使用一像素描边保持底槽上的对比度。"""
    selected = font(options['font_size'])
    draw.text(options['center'], text, font=selected, anchor='mm',
              fill=options['color'], stroke_width=options.get('stroke', 0),
              stroke_fill=options.get('stroke_color', (0, 0, 0)))


def draw_bolt(draw, size):
    """用像素多边形表示充电，避免闪电字体缺字或缩放模糊。"""
    unit = max(1, size // BASE_ICON_SIZE)
    x, y = size - 4 * unit, unit
    draw.polygon([(x + 2 * unit, y), (x, y + 3 * unit), (x + unit, y + 3 * unit),
                  (x, y + 6 * unit), (x + 3 * unit, y + 2 * unit), (x + unit, y + 2 * unit)],
                 fill=(250, 204, 21))


def draw_number(draw, options):
    """纯数字占满通知区，100% 使用专项三位数字字号。"""
    size, grid, text = options['size'], options['grid'], options['text']
    color = options['color']
    if not options['dark']:
        color = tuple(round(channel / 2) for channel in color)
    draw_text(draw, text, {'center': (size // 2, size // 2),
                           'font_size': grid[0] if len(text) <= 2 else grid[1], 'color': color})


def draw_battery(draw, options):
    """胶囊和竖向电池分别按目标网格绘制轮廓与有效电量。"""
    vertical = options['style'] == 'vertical'
    size, grid = options['size'], options['grid']
    box = grid[5] if vertical else grid[4]
    left, top, right, bottom = box
    shell = (255, 255, 255) if options['dark'] else (32, 32, 32)
    radius = max(1, size // 8)
    draw.rounded_rectangle(box, radius=radius, fill=(64, 64, 64, 180))
    if options['percentage'] >= 0:
        fill_battery(draw, box, options)
    draw.rounded_rectangle(box, radius=radius, outline=shell, width=max(1, size // BASE_ICON_SIZE))
    if vertical:
        draw.rectangle((left + 3, max(0, top - 2), right - 3, top), fill=shell)
    else:
        draw.rectangle((right + 1, (top + bottom) // 2 - 1, size - 1, (top + bottom) // 2 + 1), fill=shell)
    draw_text(draw, options['text'], {'center': ((left + right) // 2, (top + bottom) // 2),
        'font_size': (grid[3] if vertical else grid[2]) - (2 if len(options['text']) > 2 else 0),
        'color': (255, 255, 255), 'stroke': 1, 'stroke_color': (0, 0, 0)})


def fill_battery(draw, box, options):
    """0% 不画填充，横向从左到右、竖向从下到上增长。"""
    left, top, right, bottom = box
    percentage = options['percentage']
    if percentage == 0:
        return
    if options['style'] == 'vertical':
        height = round((bottom - top - 2) * percentage / 100)
        if height == 0:
            return
        fill = (left + 1, max(top + 1, bottom - height), right - 1, bottom - 1)
    else:
        width = round((right - left - 2) * percentage / 100)
        if width == 0:
            return
        fill = (left + 1, top + 1, min(right - 1, left + 1 + width), bottom - 1)
    draw.rectangle(fill, fill=options['color'])


def draw_circle(draw, options):
    """保留经典圆环入口，同样在目标尺寸直接绘制。"""
    size = options['size']
    width = max(1, size // 8)
    box = (1, 1, size - 2, size - 2)
    draw.ellipse(box, outline=(80, 80, 80), width=width)
    if options['percentage'] > 0:
        draw.arc(box, -90, -90 + round(360 * options['percentage'] / 100),
                 fill=options['color'], width=width)
    draw_text(draw, options['text'], {'center': (size // 2, size // 2),
        'font_size': options['grid'][3], 'color': (255, 255, 255)})


def create_battery_icon(percentage=-1, charging=False, *, style=TRAY_ICON_STYLE_CAPSULE,
                        is_dark_taskbar=True, size=None):
    """生成当前任务栏目标尺寸的 RGBA 图标，供 pystray 直接提交。"""
    size = taskbar_icon_size() if size is None else size
    if percentage < -1 or percentage > 100 or size <= 0:
        raise ValueError('非法电量或托盘图标尺寸')
    image = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    options = dict(size=size, grid=pixel_grid(size), style=style, dark=is_dark_taskbar,
                   percentage=percentage, text='--' if percentage < 0 else str(percentage),
                   color=battery_color(percentage, charging))
    draw = ImageDraw.Draw(image)
    if style == 'number':
        draw_number(draw, options)
    elif style == 'circle':
        draw_circle(draw, options)
    elif style in ('capsule', 'vertical'):
        draw_battery(draw, options)
    else:
        raise ValueError(f'非法托盘图标样式：{style}')
    if charging:
        draw_bolt(draw, size)
    return image
