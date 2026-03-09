# ==================== 环境检测与驱动管理 ====================
import os
import sys
import platform
import tempfile
import shutil
import subprocess
import time
import zipfile
import urllib.request
from typing import Optional, Tuple, List
from pathlib import Path


class EnvironmentChecker:
    """环境检测器"""

    EDGE_DOWNLOAD_URL = "https://go.microsoft.com/fwlink/?linkid=2108834&Channel=Stable&language=zh-cn"
    DRIVER_DOWNLOAD_URL = "https://developer.microsoft.com/en-us/microsoft-edge/tools/webdriver/"
    FFMPEG_DOWNLOAD_URL = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"

    def __init__(self):
        self.issues: List[str] = []
        self.warnings: List[str] = []
        self.edge_path: Optional[str] = None
        self.edge_version: Optional[str] = None
        self.ffmpeg_path: Optional[str] = None
        self.ffmpeg_in_path: bool = False

    def check_all(self) -> bool:
        """执行完整环境检查"""
        print("🔍 正在检查运行环境...\n")

        # 检查系统
        self._check_system()

        # 检查 Edge 浏览器
        self._check_edge_browser()

        # 检查 FFmpeg（新增）
        self._check_ffmpeg()

        # 检查网络
        self._check_network()

        # 检查结果
        if self.issues:
            print("\n" + "=" * 60)
            print("❌ 发现以下问题需要修复：")
            print("=" * 60)
            for issue in self.issues:
                print(f"   {issue}")

            if self.warnings:
                print("\n⚠️  警告（可忽略）：")
                for warning in self.warnings:
                    print(f"   {warning}")

            return False

        if self.warnings:
            print("\n⚠️  警告（可忽略）：")
            for warning in self.warnings:
                print(f"   {warning}")

        print("\n✅ 环境检查通过！\n")
        return True

    def _check_system(self):
        """检查系统信息"""
        print(f"   操作系统: {platform.system()} {platform.release()}")
        print(f"   架构: {platform.machine()}")
        print(f"   Python: {platform.python_version()}")

        if platform.system() != "Windows":
            self.warnings.append("非 Windows 系统，可能无法正常使用 Edge")

    def _check_edge_browser(self):
        """检查 Edge 浏览器"""
        possible_paths = [
            r'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe',
            r'C:\Program Files\Microsoft\Edge\Application\msedge.exe',
            os.path.expandvars(r'%LOCALAPPDATA%\Microsoft\Edge\Application\msedge.exe'),
            os.path.expandvars(r'%PROGRAMFILES%\Microsoft\Edge\Application\msedge.exe'),
            os.path.expandvars(r'%PROGRAMFILES(X86)%\Microsoft\Edge\Application\msedge.exe'),
        ]

        for path in possible_paths:
            if os.path.exists(path):
                self.edge_path = path
                self.edge_version = self._get_edge_version(path)
                print(f"   ✅ Edge 浏览器: {self.edge_version}")
                return

        self.issues.append("❌ 未检测到 Microsoft Edge 浏览器")

    def _get_edge_version(self, edge_path: str) -> str:
        """获取 Edge 版本"""
        try:
            import win32api
            info = win32api.GetFileVersionInfo(edge_path, '\\')
            version = f"{info['FileVersionMS'] >> 16}.{info['FileVersionMS'] & 0xFFFF}.{info['FileVersionLS'] >> 16}.{info['FileVersionLS'] & 0xFFFF}"
            return version
        except:
            try:
                import winreg
                key = winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                                     r"Software\Microsoft\Edge\BLBeacon")
                version, _ = winreg.QueryValueEx(key, "version")
                return version
            except:
                return "未知版本"

    # ==================== 新增：FFmpeg 检测 ====================

    def _check_ffmpeg(self):
        """检查 FFmpeg 安装和 Path 配置"""
        print("   检查 FFmpeg...")

        # 方法1：检查系统 PATH 中是否有 ffmpeg
        ffmpeg_in_path = shutil.which("ffmpeg")
        if ffmpeg_in_path:
            self.ffmpeg_path = ffmpeg_in_path
            self.ffmpeg_in_path = True
            version = self._get_ffmpeg_version(ffmpeg_in_path)
            print(f"      ✅ FFmpeg 已添加到 PATH: {version}")
            return

        # 方法2：检查常见安装路径
        common_paths = [
            r'C:\ffmpeg\bin\ffmpeg.exe',
            r'C:\Program Files\ffmpeg\bin\ffmpeg.exe',
            r'C:\Program Files (x86)\ffmpeg\bin\ffmpeg.exe',
            os.path.expandvars(r'%LOCALAPPDATA%\ffmpeg\bin\ffmpeg.exe'),
            os.path.expandvars(r'%USERPROFILE%\ffmpeg\bin\ffmpeg.exe'),
            os.path.expandvars(r'%SystemDrive%\ffmpeg\bin\ffmpeg.exe'),
        ]

        for path in common_paths:
            if os.path.exists(path):
                self.ffmpeg_path = path
                self.ffmpeg_in_path = False  # 安装了但未添加到 PATH
                version = self._get_ffmpeg_version(path)
                print(f"      ⚠️  FFmpeg 已安装但未添加到 PATH: {version}")
                print(f"         路径: {path}")
                self.issues.append("⚠️  FFmpeg 已安装但未添加到系统环境变量 PATH")
                return

        # 未安装
        self.issues.append("❌ 未检测到 FFmpeg（语音识别需要）")

    def _get_ffmpeg_version(self, ffmpeg_path: str) -> str:
        """获取 FFmpeg 版本"""
        try:
            result = subprocess.run(
                [ffmpeg_path, "-version"],
                capture_output=True,
                text=True,
                timeout=5
            )
            # 第一行格式: ffmpeg version 6.0-full_build-www.gyan.dev
            first_line = result.stdout.split('\n')[0]
            version = first_line.split()[2] if len(first_line.split()) > 2 else "未知版本"
            return version
        except:
            return "未知版本"

    def _check_network(self):
        """检查网络连接"""
        try:
            urllib.request.urlopen('https://msedgedriver.azureedge.net', timeout=5)
            print("   ✅ 网络连接正常")
        except:
            self.warnings.append("⚠️  网络连接异常，无法自动下载组件")

    # ==================== 修复选项（添加 FFmpeg 选项）====================

    def show_fix_guide(self):
        """显示修复指南"""
        print("\n" + "=" * 60)
        print("🔧 自动修复选项")
        print("=" * 60)
        print("\n请选择操作：")
        print("   [1] 自动下载并安装 Edge 浏览器")
        print("   [2] 自动下载匹配版本的 Edge 驱动")
        print("   [3] 自动下载并安装 FFmpeg（推荐）")  # 新增
        print("   [4] 将 FFmpeg 添加到系统 PATH（如已安装）")  # 新增
        print("   [5] 手动指定 Edge/驱动/FFmpeg 路径")
        print("   [6] 显示详细帮助后退出")
        print("   [Q] 退出程序")

        choice = input("\n请输入选项 (1/2/3/4/5/6/Q): ").strip().upper()
        return choice

    # ==================== 新增：FFmpeg 自动安装 ====================

    def auto_install_ffmpeg(self) -> bool:
        """自动下载并安装 FFmpeg"""
        print("\n🌐 正在下载 FFmpeg...")
        print("   来源: https://github.com/BtbN/FFmpeg-Builds")

        try:
            # 下载路径
            zip_path = os.path.join(tempfile.gettempdir(), "ffmpeg.zip")

            # 显示进度
            def download_progress(block_num, block_size, total_size):
                downloaded = block_num * block_size
                percent = min(100, downloaded * 100 / total_size)
                print(f"\r   进度: {percent:.1f}% ({downloaded // 1024 // 1024}MB / {total_size // 1024 // 1024}MB)",
                      end="")

            print("   下载中（约 130MB，请耐心等待）...")
            urllib.request.urlretrieve(
                self.FFMPEG_DOWNLOAD_URL,
                zip_path,
                reporthook=download_progress
            )
            print()  # 换行

            # 解压
            print("   📦 解压文件...")
            extract_dir = os.path.expandvars(r'%LOCALAPPDATA%\ffmpeg')

            # 清理旧版本
            if os.path.exists(extract_dir):
                shutil.rmtree(extract_dir, ignore_errors=True)

            os.makedirs(extract_dir, exist_ok=True)

            with zipfile.ZipFile(zip_path, 'r') as zip_ref:
                zip_ref.extractall(extract_dir)

            # 找到实际的 bin 目录
            bin_dir = None
            for root, dirs, files in os.walk(extract_dir):
                if 'ffmpeg.exe' in files and 'bin' in root:
                    bin_dir = root
                    break

            if not bin_dir:
                # 可能是直接解压到子目录
                subdirs = [d for d in os.listdir(extract_dir) if os.path.isdir(os.path.join(extract_dir, d))]
                if subdirs:
                    potential_bin = os.path.join(extract_dir, subdirs[0], 'bin')
                    if os.path.exists(os.path.join(potential_bin, 'ffmpeg.exe')):
                        bin_dir = potential_bin

            if not bin_dir:
                print("   ❌ 解压后未找到 ffmpeg.exe")
                return False

            # 添加到系统 PATH
            print("   🔧 添加到系统环境变量...")
            self._add_to_system_path(bin_dir)

            # 验证
            print("   ✅ 验证安装...")
            # 刷新环境变量
            os.environ['PATH'] = bin_dir + os.pathsep + os.environ.get('PATH', '')

            ffmpeg_exe = os.path.join(bin_dir, 'ffmpeg.exe')
            version = self._get_ffmpeg_version(ffmpeg_exe)
            print(f"   ✅ FFmpeg 安装成功: {version}")
            print(f"      路径: {bin_dir}")

            # 清理下载文件
            try:
                os.remove(zip_path)
            except:
                pass

            print("\n⚠️  请重新运行本程序以加载新的环境变量")
            input("按回车键退出...")
            return True

        except Exception as e:
            print(f"\n   ❌ 安装失败: {str(e)[:100]}")
            print("   💡 请手动下载: https://ffmpeg.org/download.html")
            return False

    def add_ffmpeg_to_path(self) -> bool:
        """将已安装的 FFmpeg 添加到系统 PATH"""
        if not self.ffmpeg_path:
            print("   ❌ 未检测到已安装的 FFmpeg")
            return False

        # 找到 bin 目录
        bin_dir = os.path.dirname(self.ffmpeg_path)

        print(f"\n🔧 正在添加到系统 PATH...")
        print(f"   路径: {bin_dir}")

        try:
            self._add_to_system_path(bin_dir)
            print("   ✅ 添加成功")
            print("\n⚠️  请重新运行本程序以加载新的环境变量")
            input("按回车键退出...")
            return True
        except Exception as e:
            print(f"   ❌ 添加失败: {str(e)[:50]}")
            return False

    def _add_to_system_path(self, bin_dir: str):
        """将目录添加到系统 PATH 环境变量"""
        import winreg

        # 获取当前系统 PATH
        key = winreg.OpenKey(
            winreg.HKEY_CURRENT_USER,
            r"Environment",
            0,
            winreg.KEY_READ | winreg.KEY_WRITE
        )

        try:
            current_path, _ = winreg.QueryValueEx(key, "Path")
        except:
            current_path = ""

        # 检查是否已存在
        paths = [p.strip() for p in current_path.split(os.pathsep) if p.strip()]
        if bin_dir in paths:
            print("   ℹ️  该路径已在 PATH 中")
            return

        # 添加新路径
        new_path = bin_dir + os.pathsep + current_path if current_path else bin_dir
        winreg.SetValueEx(key, "Path", 0, winreg.REG_EXPAND_SZ, new_path)
        winreg.CloseKey(key)

        # 通知系统环境变量已更改
        import ctypes
        HWND_BROADCAST = 0xFFFF
        WM_SETTINGCHANGE = 0x1A
        SMTO_ABORTIFHUNG = 0x0002
        result = ctypes.c_long()
        ctypes.windll.user32.SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            "Environment",
            SMTO_ABORTIFHUNG,
            5000,
            ctypes.byref(result)
        )

        print("   ✅ 系统环境变量已更新")

    # ==================== 其他修复选项（保持原有）====================

    def auto_install_edge(self) -> bool:
        """自动安装 Edge"""
        print("\n🌐 正在下载 Edge 浏览器安装程序...")
        try:
            installer_path = os.path.join(tempfile.gettempdir(), "EdgeSetup.exe")
            urllib.request.urlretrieve(self.EDGE_DOWNLOAD_URL, installer_path)
            print(f"   ✅ 下载完成")
            print("   🚀 启动安装程序...")
            subprocess.Popen([installer_path], shell=True)
            print("\n⚠️  请完成 Edge 安装后，重新运行本程序")
            input("按回车键退出...")
            return False
        except Exception as e:
            print(f"   ❌ 下载失败: {str(e)[:50]}")
            return False

    def auto_download_driver(self, target_dir: str) -> Optional[str]:
        """自动下载匹配版本的驱动"""
        if not self.edge_version:
            print("   ❌ 无法确定 Edge 版本")
            return None

        print(f"\n🌐 正在下载 Edge 驱动（版本 {self.edge_version}）...")

        try:
            major_version = self.edge_version.split('.')[0]
            zip_path = os.path.join(tempfile.gettempdir(), "edgedriver.zip")

            # 尝试精确版本
            download_url = f"https://msedgedriver.azureedge.net/{self.edge_version}/edgedriver_win64.zip"

            try:
                urllib.request.urlretrieve(download_url, zip_path)
            except:
                print(f"   ⚠️ 精确版本下载失败，尝试主版本号...")
                version_url = f"https://msedgedriver.azureedge.net/LATEST_RELEASE_{major_version}_WINDOWS"
                with urllib.request.urlopen(version_url) as response:
                    latest_version = response.read().decode('utf-8').strip()
                download_url = f"https://msedgedriver.azureedge.net/{latest_version}/edgedriver_win64.zip"
                urllib.request.urlretrieve(download_url, zip_path)

            print("   📦 解压驱动...")
            with zipfile.ZipFile(zip_path, 'r') as zip_ref:
                zip_ref.extract("msedgedriver.exe", target_dir)

            os.remove(zip_path)

            driver_path = os.path.join(target_dir, "msedgedriver.exe")
            print(f"   ✅ 驱动已保存: {driver_path}")
            return driver_path

        except Exception as e:
            print(f"   ❌ 下载失败: {str(e)[:50]}")
            return None

    def manual_specify_path(self) -> Tuple[Optional[str], Optional[str], Optional[str]]:
        """手动指定路径（添加 FFmpeg）"""
        print("\n" + "-" * 60)
        print("手动指定路径")
        print("-" * 60)

        # Edge 路径
        edge_path = input("Edge 浏览器路径（直接回车跳过）: ").strip()
        if edge_path and not os.path.exists(edge_path):
            print("   ⚠️ 路径不存在")
            edge_path = None

        # 驱动路径
        driver_path = input("msedgedriver.exe 路径（直接回车跳过）: ").strip()
        if driver_path and not os.path.exists(driver_path):
            print("   ⚠️ 路径不存在")
            driver_path = None

        # FFmpeg 路径（新增）
        ffmpeg_path = input("ffmpeg.exe 路径（直接回车跳过）: ").strip()
        if ffmpeg_path and not os.path.exists(ffmpeg_path):
            print("   ⚠️ 路径不存在")
            ffmpeg_path = None

        return edge_path, driver_path, ffmpeg_path


class DriverManager:
    """驱动管理器（保持原有）"""

    def __init__(self):
        self.bundled_driver = self._find_bundled_driver()
        self.downloaded_driver = self._find_downloaded_driver()

    def _find_bundled_driver(self) -> Optional[str]:
        possible_paths = [
            get_resource_path('msedgedriver.exe'),
            get_resource_path('driver/msedgedriver.exe'),
            os.path.join(os.path.dirname(sys.executable), 'msedgedriver.exe'),
        ]

        for path in possible_paths:
            if path and os.path.exists(path):
                return path
        return None

    def _find_downloaded_driver(self) -> Optional[str]:
        app_data = os.path.expandvars(r'%LOCALAPPDATA%\U校园AI答题')
        driver_path = os.path.join(app_data, 'msedgedriver.exe')

        if os.path.exists(driver_path):
            return driver_path
        return None

    def get_driver_path(self) -> Optional[str]:
        return self.bundled_driver or self.downloaded_driver

    def save_driver(self, driver_path: str) -> str:
        app_data = os.path.expandvars(r'%LOCALAPPDATA%\U校园AI答题')
        os.makedirs(app_data, exist_ok=True)

        target_path = os.path.join(app_data, 'msedgedriver.exe')
        shutil.copy2(driver_path, target_path)
        return target_path


def get_resource_path(relative_path: str) -> str:
    """获取资源路径"""
    if hasattr(sys, '_MEIPASS'):
        base_path = sys._MEIPASS
    else:
        base_path = os.path.dirname(os.path.abspath(__file__))

    return os.path.join(base_path, relative_path)
