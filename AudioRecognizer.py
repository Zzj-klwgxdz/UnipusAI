import whisper
import requests
import tempfile
import os
import hashlib
from typing import Dict,Optional
from pydub import AudioSegment



class AudioTranscriber:
    """
    使用 OpenAI Whisper 进行语音识别

    支持方式：
    1. OpenAI API (需要 api_key)
    2. 本地 Whisper 模型 (无需网络，首次下载模型)
    """

    def __init__(self, api_key: Optional[str] = None, use_local: bool = False):
        """
        Args:
            api_key: OpenAI API Key，为 None 则使用本地模型
            use_local: 强制使用本地模型（无需 API）
        """
        self.api_key = api_key
        self.use_local = use_local
        self.local_model = None
        self._transcript_cache: Dict[str, str] = {}

        if use_local or api_key is None:
            self._init_local_model()

    def _init_local_model(self):
        """初始化本地 Whisper 模型"""
        try:
            print("      📦 加载 Whisper 本地模型 (base)...")
            # 可选: tiny, base, small, medium, large
            # base 是速度与准确率的平衡选择
            self.local_model = whisper.load_model("base")
            print("      ✅ 本地模型加载完成")
        except ImportError:
            print("      ⚠️ 未安装 whisper，请运行: pip install openai-whisper")
            raise
        except Exception as e:
            print(f"      ❌ 加载本地模型失败: {str(e)[:50]}")
            raise

    def transcribe(self, audio_url: str, language: str = "en") -> str:
        """
        下载音频并转录为文字

        Args:
            audio_url: 音频文件URL
            language: 语言代码，默认英语 en，中文 zh

        Returns:
            识别出的文字
        """
        # 检查缓存
        cache_key = hashlib.md5(audio_url.encode()).hexdigest()
        if cache_key in self._transcript_cache:
            print(f"      📦 使用缓存的识别结果")
            return self._transcript_cache[cache_key]

        temp_files = []

        try:
            print(f"      ⬇️  下载音频...")
            # 下载音频
            response = requests.get(audio_url, timeout=30)
            response.raise_for_status()

            # 保存原始音频
            with tempfile.NamedTemporaryFile(suffix=".mp3", delete=False) as f:
                f.write(response.content)
                audio_path = f.name
                temp_files.append(audio_path)

            print(f"      🎙️  开始识别...")

            # 选择识别方式
            if self.use_local or self.api_key is None:
                text = self._transcribe_local(audio_path, language)
            else:
                text = self._transcribe_api(audio_path, language)

            # 缓存结果
            if text:
                self._transcript_cache[cache_key] = text
                print(f"      ✅ 识别成功 ({len(text)} 字符)")
                print(f"      ✅ 音频识别结果{text}")

            return text or ""

        except requests.RequestException as e:
            print(f"      ❌ 下载音频失败: {str(e)[:50]}")
            return ""
        except Exception as e:
            print(f"      ❌ 识别失败: {str(e)[:50]}")
            return ""
        finally:
            # 清理临时文件
            for f in temp_files:
                try:
                    if os.path.exists(f):
                        os.unlink(f)
                except:
                    pass

    def _transcribe_api(self, audio_path: str, language: str) -> Optional[str]:
        """使用 OpenAI API 识别"""
        try:
            from openai import OpenAI

            client = OpenAI(api_key=self.api_key)

            with open(audio_path, "rb") as audio_file:
                transcript = client.audio.transcriptions.create(
                    model="whisper-1",
                    file=audio_file,
                    language=language,
                    response_format="text"
                )

            return transcript.strip() if transcript else None

        except Exception as e:
            print(f"      ⚠️ API 识别失败，尝试本地模型: {str(e)[:50]}")
            if self.local_model is None:
                self._init_local_model()
            return self._transcribe_local(audio_path, language)

    def _transcribe_local(self, audio_path: str, language: str) -> Optional[str]:
        """使用本地 Whisper 模型识别"""
        if self.local_model is None:
            print("      ❌ 本地模型未加载")
            return None

        try:
            # 执行识别
            result = self.local_model.transcribe(
                audio_path,
                language=language,
                fp16=False  # CPU 运行设为 False
            )

            return result["text"].strip() if result else None

        except Exception as e:
            print(f"      ❌ 本地识别失败: {str(e)[:50]}")
            return None

    def transcribe_long_audio(self, audio_url: str, language: str = "en",
                              chunk_length: int = 30) -> str:
        """
        识别长音频（优先使用本地模型直接处理，避免分段）
        """
        temp_files = []
        try:
            # 下载音频
            print(f"      ⬇️ 下载音频...")
            response = requests.get(audio_url, timeout=60)  # 增加超时时间
            response.raise_for_status()

            # 保存音频
            with tempfile.NamedTemporaryFile(suffix=".mp3", delete=False) as f:
                f.write(response.content)
                audio_path = f.name
                temp_files.append(audio_path)

            file_size = os.path.getsize(audio_path)
            print(f"      📊 音频大小: {file_size / 1024 / 1024:.1f} MB")

            # 关键修改：本地模型直接识别整个文件，不分段
            if self.use_local or self.api_key is None:
                print(f"      🎙️ 使用本地模型直接识别（适合长音频）...")
                return self._transcribe_local(audio_path, language) or ""

            # 如果需要用 API 且文件小于 25MB，直接 API 调用
            if file_size < 25 * 1024 * 1024:
                return self._transcribe_api(audio_path, language) or ""

            # API 但文件超大时才分段（需要 pydub+ffmpeg）
            print(f"      ⏭️ API 文件过大，尝试分段...")
            return self._split_and_transcribe(audio_path, language, chunk_length)

        except Exception as e:
            print(f"      ❌ 音频处理失败: {str(e)[:80]}")
            return ""
        finally:
            for f in temp_files:
                try:
                    if os.path.exists(f):
                        os.unlink(f)
                except:
                    pass

    def _split_and_transcribe(self, audio_path: str, language: str,
                              chunk_length: int) -> str:
        """分段识别（带故障回退）"""
        try:

            # 测试 pydub 是否可用（ffmpeg 检查）
            AudioSegment.converter  # 简单属性检查
        except Exception as e:
            print(f"      ⚠️ pydub/ffmpeg 不可用: {e}，回退到直接识别")
            if self.local_model:
                return self._transcribe_local(audio_path, language) or ""
            return self._transcribe_api(audio_path, language) or ""

        try:
            audio = AudioSegment.from_mp3(audio_path)
        except Exception as e:
            print(f"      ❌ 分段识别出错: {str(e)[:80]}，回退到直接识别")
            if self.local_model:
                return self._transcribe_local(audio_path, language) or ""
            # 最后尝试 API 直接上
            try:
                return self._transcribe_api(audio_path, language) or ""
            except:
                return ""
