from AudioRecognizer import *
from EnvironmentChecker import *
import hashlib, json, logging, os, sys, random, re, threading, time, warnings,winsound
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum, auto
from glob import glob
from typing import List, Optional, Dict, Any, Tuple, Callable

from openai import OpenAI
from selenium import webdriver
from selenium.common.exceptions import NoSuchElementException, TimeoutException, StaleElementReferenceException
from selenium.webdriver.common.action_chains import ActionChains
from selenium.webdriver.common.by import By
from selenium.webdriver.support import expected_conditions as EC
from selenium.webdriver.support.ui import WebDriverWait



# ==================== 标记弃用方法 ====================
def deprecated(func):
    def wrapper(*args, **kwargs):
        warnings.warn(f"Function {func.__name__} is deprecated and will be removed in future versions.",
                      DeprecationWarning, stacklevel=2)
        return func(*args, **kwargs)

    return wrapper


# ==================== 日志配置 ====================
def setup_logging():
    """配置日志系统：控制台简洁输出 + 文件详细记录"""

    def clean_all_logs(log_dir):
        """清空所有旧日志（激进模式）"""
        try:
            log_pattern = os.path.join(log_dir, 'ucampus_*.log')
            log_files = glob(log_pattern)
            for old_file in log_files:
                try:
                    os.remove(old_file)
                except:
                    pass
        except Exception:
            pass

    # 创建logger
    logger = logging.getLogger('UCampusBot')
    logger.setLevel(logging.DEBUG)
    # 清除已有处理器
    logger.handlers = []
    # 创建日志目录
    log_dir = os.path.join(os.path.dirname(__file__), 'logs')
    os.makedirs(log_dir, exist_ok=True)
    # 清除上一个日志
    clean_all_logs(log_dir)
    # 日志文件名：logs/ucampus_2024-02-08_14-30-25.log
    log_file = os.path.join(log_dir, f'ucampus_{datetime.now().strftime("%Y-%m-%d_%H-%M-%S")}.log')

    # === 文件处理器：记录所有信息（DEBUG及以上）===
    file_handler = logging.FileHandler(log_file, encoding='utf-8')
    file_handler.setLevel(logging.DEBUG)
    file_formatter = logging.Formatter(
        '[%(asctime)s] [%(levelname)s] [%(funcName)s:%(lineno)d]\n%(message)s\n',
        datefmt='%H:%M:%S'
    )
    file_handler.setFormatter(file_formatter)
    logger.addHandler(file_handler)

    # === 控制台处理器：只显示简洁信息（INFO及以上，过滤掉报错堆栈）===
    console_handler = logging.StreamHandler(sys.stdout)
    console_handler.setLevel(logging.INFO)

    # 自定义过滤器：只显示用户关心的操作信息
    class ConsoleFilter(logging.Filter):
        def filter(self, record):
            # 只显示特定级别的消息
            if record.levelno >= logging.ERROR:
                # 错误只显示简短信息，不显示堆栈
                record.msg = f"❌ {record.msg}"
            return True

    console_handler.addFilter(ConsoleFilter())
    console_formatter = logging.Formatter('%(message)s')
    console_handler.setFormatter(console_formatter)
    logger.addHandler(console_handler)

    # 重定向print到日志系统
    class PrintRedirector:
        def __init__(self, logger, level=logging.INFO):
            self.logger = logger
            self.level = level
            self.buffer = ""

        def write(self, text):
            if text.strip():
                # 根据内容判断级别
                if any(x in text for x in ['❌', 'Error', 'Exception', 'Traceback']):
                    self.logger.error(text.strip())
                elif any(x in text for x in ['⚠️', 'Warning']):
                    self.logger.warning(text.strip())
                else:
                    self.logger.info(text.strip())

        def flush(self):
            pass

    # 保存原始stdout，用于真正需要控制台输出的情况
    sys._original_stdout = sys.stdout
    # 重定向print
    sys.stdout = PrintRedirector(logger)
    return logger, log_file


# ==================== 配置管理 ====================
@dataclass(frozen=True)
class Config:
    """不可变配置类"""
    url: str
    username: str
    password: str
    api_key: str
    token_full: str
    target_course: str
    learning_strategy: str
    base_url: str
    model: str
    temperature: float
    max_tokens: int
    timeout: int
    whisper_api: str

    @classmethod
    def from_json(cls, path: str = "config.json") -> "Config":
        with open(path, "r", encoding="UTF-8") as f:
            data = json.load(f)
        return cls(
            url=data.get("url"),
            username=data.get("username"),
            password=data.get("password"),
            token_full=data.get("token_full"),
            api_key=data.get("api_key"),
            target_course=data.get("target_course", "新视野大学英语（第四版）读写教程1"),
            learning_strategy=data.get("learning_strategy", "learn_all_compulsory_course"),
            base_url=data.get("base_url", "https://api.moonshot.cn/v1"),
            model=data.get("model", "kimi-k2-turbo-preview"),
            temperature=data.get("temperature", 0.3),
            max_tokens=data.get("max_tokens", 2000),
            timeout=data.get("timeout", 10),
            whisper_api=data.get("whisper_api", None)

        )


# ==================== 题目类型模型 ====================
class QuestionType(Enum):
    """题目类型"""
    SINGLE_CHOICE = auto()
    MULTIPLE_CHOICE = auto()
    FILL_IN = auto()
    TEXT = auto()
    SORTING = auto()
    DROPDOWN = auto()
    VOCABULARY_FLASHCARD = auto()
    BANKED_CLOZE = auto()
    VOCABULARY_TEST = auto()  # 词汇测试（英汉互译）
    DISCUSSION_BOARD = auto()
    DROPDOWN_SELECT = auto()
    LISTENING_FILL_IN = auto()
    UNKNOWN = auto()


@dataclass
class Option:
    """选项"""
    letter: str
    text: str
    element: Any
    is_selected: bool = False


@dataclass
class Question:
    """题目"""
    number: int
    text: str
    q_type: QuestionType
    element: Any
    options: List[Option] = field(default_factory=list)
    inputs: List[Any] = field(default_factory=list)
    banked_options: List[str] = field(default_factory=list)
    banked_blanks: List[Dict] = field(default_factory=list)
    directions: str = ""

    def is_interactive(self) -> bool:
        """是否有交互元素"""
        if self.q_type in [QuestionType.VOCABULARY_FLASHCARD,  # 闪卡需要交互
                           QuestionType.DISCUSSION_BOARD]:
            return True
        return bool(self.options or self.inputs or self.banked_blanks)

    @property
    def is_phrase_mode(self) -> bool:
        """检测是短语填空还是单词填空"""
        if not self.banked_options:
            return False
        phrase_count = sum(1 for opt in self.banked_options if ' ' in opt.strip() or len(opt) > 15)
        return phrase_count / len(self.banked_options) > 0.3


@dataclass
class AnswerResult:
    """答题结果"""
    success: bool
    question_number: int
    answer: str
    message: str = ""


# ==================== 选择器仓库（集中管理） ====================
class Selectors:
    """CSS选择器仓库"""
    # 填空题：input 在 .fe-scoop 内，无 material 容器
    FILL_BLANK_INPUTS = [
        '.fe-scoop input:not([type="hidden"])',  # 严格限定input
        '.comp-abs-input input',
        'input.fill-blank--bc-input-DelG1',
    ]
    # 写作题：textarea，有 material 容器
    TEXTAREA_INPUTS = [
        'textarea.question-textarea-content',
        'textarea.writing--textarea-36VPs',
    ]
    # 材料容器（写作题标志）
    MATERIAL_CONTAINER = '.layout-material-container'
    QUESTION_CONTAINERS = [
        '.question-common-abs-reply',
        '.question-common-abs-banked-cloze',
        '.question-wrap',
        '.question-basic',
        '.layoutBody-container.has-reply',
        '.question-material-banked-cloze.question-abs-question',
        '.itest-section',
        '.oral-study-sentence',
        '.question-common-abs-choice',
        '.question-vocabulary',
        '.vocContainer',
    ]
    CHOICE_OPTIONS = [
        '.option.isNotReview',
        'div.option',
        '.MultipleChoice--checkbox-item-34A_-',
        'ul[class*="single-choice"] li label',
        '.option-wrap',
    ]
    OPTION_CAPTION = ['.caption', 'span[class*="index"]', '.MultipleChoice--checkbox-opt-2F4xY']
    OPTION_CONTENT = ['.component-htmlview.content', 'div.html-view[class*="content"]', '.html-view', '.content', 'p']
    FILL_INPUTS = [
        'input.fill-blank--bc-input-DelG1',
        '.fe-scoop input:not([type="hidden"])',
        '.comp-abs-input input',
        'textarea.question-inputbox-input',
        '.question-inputbox-input',
        'textarea.question-textarea-content',
        'textarea.writing--textarea-36VPs',
        'textarea.scoopFill_textarea',
        '.blankinput',
        'input[type="text"]',
    ]
    TEXTAREAS = [
        'textarea.writing--textarea-36VPs',
        'textarea.scoopFill_textarea',
        'textarea.question-inputbox-input',
        '.question-inputbox-input-container textarea',
        'textarea.question-textarea-content',
    ]
    QUESTION_TITLE = [
        '.ques-title',
        '.component-htmlview.ques-title',
        '.question-inputbox-header',
        '.component-htmlview',
        '.title',
        'p',
        '.question-stem',
    ]
    SUBMIT_BUTTON = [
        'button[type="submit"]',
        'button[class*="submit"]',
        'button[class*="confirm"]',
        '.submit-bar-pc--btn-1_Xvo',
        '.btns-submit button.submit-btn',
        'button.submit-btn',
        '.btn',
    ]
    VIDEO = ['video.vjs-tech', 'video']
    VOCABULARY_ACTIONS = ['.vocActions', '.vocabulary-actions']
    BANKED_OPTIONS = [
        '.question-material-banked-cloze-reply .option-wrapper .option',
        '.banked-options .option',
        '[data-rbd-draggable-id^="options-"]'
    ]
    BANKED_BLANKS = ['.fe-scoop', '.scoop-wrapper', '.comp-abs-input']
    # Tab导航
    LEVEL1_TABS = [
        '.pc-header-tabs-container .pc-tab-row > .tab',
        '.pc-header-tabs-container .ant-col.tab',
        '.pc-tab-row > [class*="pc-header-tab"]',
    ]
    LEVEL2_TABS = [
        '.pc-header-tasks-row > .pc-task',
        ':scope > div > div > .pc-header-tasks-row > .pc-task',
    ]
    # 侧边栏
    SIDEBAR = [
        '.pc-slider-content-menu',
        '.pc-slier-menu-container',
        '.pc-slider-menu',
        '#sidemenu',
        '.menu--u3menu-3Xu4h',
        '[class*="slider-menu"]',
        '[class*="side-menu"]'
    ]
    SIDEBAR_NODES = [
        'div[data-role="node"]',
        'div[data-role="micro"]',
        'li.group.courseware',
        '.pc-menu-node',
        '[class*="menu-node"]',
        '.group.courseware'
    ]


# ==================== 工具类 ====================
class WebDriverHelper:
    """WebDriver辅助工具类（静态方法）"""

    @staticmethod
    def safe_find_element(driver, selectors: List[str], parent=None, timeout: int = 5) -> Optional[Any]:
        """安全查找单个元素"""
        search_context = parent if parent else driver
        wait = WebDriverWait(search_context, timeout)
        for selector in selectors:
            try:
                element = wait.until(EC.presence_of_element_located((By.CSS_SELECTOR, selector)))
                if element.is_displayed():
                    return element
            except (TimeoutException, NoSuchElementException):
                continue
        return None

    @staticmethod
    def safe_find_elements(driver, selectors: List[str], parent=None, visible_only: bool = True) -> List[Any]:
        """安全查找多个元素"""
        search_context = parent if parent else driver
        for selector in selectors:
            try:
                elements = search_context.find_elements(By.CSS_SELECTOR, selector)
                if visible_only:
                    elements = [e for e in elements if e.is_displayed()]
                if elements:
                    return elements
            except Exception as e:
                error_msg = str(e)
                print(f"操作失败: {error_msg[:50]}")  # 控制台只显示简短信息
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
                continue
        return []

    @staticmethod
    def is_in_viewport(driver, element) -> bool:
        """检查元素是否在视口内"""
        try:
            return driver.execute_script("""
                var rect = arguments[0].getBoundingClientRect();
                var html = document.documentElement;
                return (
                    rect.top >= 0 && rect.left >= 0 &&
                    rect.bottom <= (window.innerHeight || html.clientHeight) &&
                    rect.right <= (window.innerWidth || html.clientWidth)
                );
            """, element)
        except:
            return True

    @staticmethod
    def human_like_delay(base_delay: float = 0.1) -> None:
        """随机延迟"""
        delay = base_delay * (0.8 + random.random() * 0.4)
        time.sleep(delay)

    @staticmethod
    def simulate_typing(driver, element, text: str) -> None:
        """模拟人类打字"""
        actions = ActionChains(driver)
        actions.move_to_element(element).click().perform()
        WebDriverHelper.human_like_delay(0.1)
        element.clear()
        WebDriverHelper.human_like_delay(0.1)
        for char in text:
            element.send_keys(char)
            time.sleep(random.uniform(0.01, 0.05))
        # 触发事件
        driver.execute_script("""
            arguments[0].dispatchEvent(new Event('input', {bubbles: true}));
            arguments[0].dispatchEvent(new Event('change', {bubbles: true}));
            arguments[0].dispatchEvent(new Event('blur', {bubbles: true}));
        """, element)
        WebDriverHelper.human_like_delay(0.1)

    @staticmethod
    def safe_click(driver, element, retries: int = 3) -> bool:
        """安全点击元素"""
        for i in range(retries):
            try:
                # 滚动到可视区域
                driver.execute_script(
                    "arguments[0].scrollIntoView({block: 'center', behavior: 'smooth'});",
                    element
                )
                time.sleep(0.3)

                # 尝试点击
                try:
                    element.click()
                except:
                    driver.execute_script("arguments[0].click();", element)
                return True

            except StaleElementReferenceException:
                if i < retries - 1:
                    time.sleep(1)
                    continue
            except Exception as e:
                if i < retries - 1:
                    time.sleep(0.5)
                    continue
                error_msg = str(e)
                print(f"操作失败: {error_msg[:50]}")  # 控制台只显示简短信息
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
        return False


# ==================== AI客户端 ====================
class KimiClient:
    """Kimi API客户端 - 职责：仅处理API通信"""

    SYSTEM_PROMPT = """你是一个专业的英语教学助手，擅长分析英语题目。
请根据题目要求给出准确答案，注意区分不同题型：
- 词汇匹配题：根据英文选中文，或根据中文选英文
- 选词填空：选择最合适的单词填入
- 阅读理解：基于文章内容作答"""

    def __init__(self, config: Config):
        self.config = config
        self.client = OpenAI(api_key=config.api_key, base_url=config.base_url)
        self.conversation_history: List[Dict] = []
        self.current_chapter_id: Optional[str] = None
        self.accumulated_passages: set = set()  # 已累积的原文哈希，防重复

    def start_new_chapter(self, chapter_id: str):
        """开始新章节，记录章节ID（不自动清空历史）"""
        # 只记录ID，不清空历史，让调用方决定是否清空
        self.current_chapter_id = chapter_id
        print(f"🔄 记录章节: {chapter_id[:50]}")

    def force_reset(self, chapter_id: str):
        """强制清空所有历史，无论章节是否相同"""
        self.conversation_history = []
        self.current_chapter_id = chapter_id
        self.accumulated_passages = set()
        print(f"🔄 强制重置章节: {chapter_id[:50]}")

    def add_passage_if_new(self, passage: str) -> bool:
        """添加原文（如果是新的），返回是否添加成功"""
        if not passage or len(passage) < 50:
            return False

        passage_hash = hashlib.md5(passage.encode()).hexdigest()[:16]

        if passage_hash in self.accumulated_passages:
            print(f"   📄 原文已存在，跳过")
            return False

        self.accumulated_passages.add(passage_hash)

        passage_msg = {
            "role": "user",
            "content": f"【阅读材料 {len(self.accumulated_passages)}】\n\n{passage}\n\n请理解以上材料，等待后续问题。"
        }
        self.conversation_history.append(passage_msg)
        self.conversation_history.append({
            "role": "assistant",
            "content": f"我已理解材料 {len(self.accumulated_passages)}。请提出问题。"
        })

        print(f"   📄 新增原文（{len(passage)}字符），当前共{len(self.accumulated_passages)}篇")
        return True

    def ask(self, prompt: str, retry_count: int = 3) -> Optional[str]:
        """发送问题并获取回答"""
        print(f"当前ai对话历史共{len(self.conversation_history)}条")
        for attempt in range(retry_count):
            try:
                messages = [
                    {"role": "system", "content": self.SYSTEM_PROMPT},
                    *self.conversation_history,
                    {"role": "user", "content": prompt}
                ]
                # 调试
                # # ========== 打印AI收到的完整请求 ==========
                # print("\n" + "=" * 60)
                # print(" 发送给AI的完整消息：")
                # print("=" * 60)
                # for i, msg in enumerate(messages):
                #     role = msg["role"]
                #     content = msg["content"]
                #     # 截断过长的内容
                #     preview = content[:500] + "..." if len(content) > 500 else content
                #     print(f"\n[{i}] {role.upper()}:")
                #     print("AI_receive"+preview)
                # print("\n" + "=" * 60)
                # print(" 当前Prompt内容：")
                # print("-" * 60)
                # prompt_preview = prompt[:800] + "..." if len(prompt) > 800 else prompt
                # print("AI_receive"+prompt_preview)
                # print("=" * 60 + "\n")
                # # ===========================================

                response = self.client.chat.completions.create(
                    model=self.config.model,
                    messages=messages,
                    temperature=self.config.temperature,
                    max_tokens=self.config.max_tokens
                )

                answer = response.choices[0].message.content.strip()

                # 保存到历史
                self.conversation_history.append({"role": "user", "content": prompt})
                self.conversation_history.append({"role": "assistant", "content": answer})

                # 限制历史长度（保留最近10轮）
                if len(self.conversation_history) > 22:
                    self.conversation_history = self.conversation_history[:2] + self.conversation_history[-20:]

                print(f"AI回答: {answer}")
                return answer

            except Exception as e:
                if attempt < retry_count - 1:
                    time.sleep((2 ** attempt) + random.random())
                error_msg = str(e)
                print(f"AI调用失败: {error_msg[:50]}")  # 控制台只显示简短信息
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件

        return None


# ==================== 题目解析策略模式 ====================
class QuestionParserStrategy(ABC):
    """题目解析策略基类"""

    @abstractmethod
    def can_parse(self, container, driver) -> bool:
        """是否能解析该容器"""
        pass

    @abstractmethod
    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        """解析题目"""
        pass


class QuestionParser:
    """题目解析器 - 使用策略模式"""

    def __init__(self, driver, whisper_api_key: Optional[str] = None):
        self.driver = driver
        # 按优先级注册策略（顺序可能影响解析优先级）
        self.strategies: List[QuestionParserStrategy] = [
            DiscussionBoardStrategy(),
            VocabularyFlashcardStrategy(),
            VocabularyTestStrategy(),
            DropdownSelectStrategy(),
            BankedClozeStrategy(),
            ListeningFillInStrategy(),   # 新版无需参数，直接实例化
            StandardChoiceStrategy(),
            TextInputStrategy(),
            FillInStrategy(),
        ]

    def _find_reading_question_containers(self) -> List[Any]:
        """
        查找需要拆分的问答题容器（阅读问答题、翻译题）
        关键：多个reply，每个包含.question-inputbox，且direction表明是多题作答
        """
        try:
            # 先检查direction，确定是否是多题作答类型
            direction_text = ""
            try:
                direction_elem = self.driver.find_element(
                    By.CSS_SELECTOR,
                    ".layout-direction-container .component-htmlview"
                )
                direction_text = direction_elem.text.lower()
            except:
                pass

            # 判断是否是多题作答类型
            is_multi_question_type = any(kw in direction_text for kw in [
                'answer', 'question', 'according to',  # 阅读问答题
                'translate',  # 翻译题
            ])

            # 如果不是多题作答类型，不拆分
            if not is_multi_question_type:
                return []

            # 查找容器
            body_selectors = [
                '.layoutBody-container.has-material.has-reply',
                '.layoutBody-container.has-reply',  # 翻译题
            ]

            for selector in body_selectors:
                body_containers = self.driver.find_elements(By.CSS_SELECTOR, selector)

                for body in body_containers:
                    # 排除写作题（有fe-scoop）
                    has_scoop = body.find_elements(By.CSS_SELECTOR, '.fe-scoop')
                    if has_scoop:
                        continue

                    # 排除选词填空
                    has_options = body.find_elements(By.CSS_SELECTOR, '.option-wrapper, .banked-options')
                    if has_options:
                        continue

                    # 查找所有包含inputbox的reply
                    reply_containers = body.find_elements(By.CSS_SELECTOR, '.question-common-abs-reply')

                    valid_replies = []
                    for reply in reply_containers:
                        try:
                            if reply.is_displayed() and reply.find_elements(By.CSS_SELECTOR, '.question-inputbox'):
                                valid_replies.append(reply)
                        except:
                            continue

                    # 只有明确多个问题，或者direction表明是多题，才拆分
                    if len(valid_replies) >= 2:
                        return valid_replies
                    # 单个问题不拆分，让大容器逻辑处理

            return []

        except Exception as e:
            logger.debug(f"查找问答题失败: {e}")
            return []

    def _extract_directions_from_page(self) -> str:
        """从页面统一提取 direction（参考 TextInputStrategy 的策略）"""
        try:
            # 策略1：查找 .layout-direction-container（最可靠）
            direction_elem = self.driver.find_element(
                By.CSS_SELECTOR,
                ".layout-direction-container .component-htmlview"
            )
            return direction_elem.text.strip()
        except:
            pass

        try:
            # 策略2：查找 .abs-direction
            direction_elem = self.driver.find_element(
                By.CSS_SELECTOR,
                ".abs-direction .content"
            )
            return direction_elem.text.strip()
        except:
            pass

        try:
            # 策略3：查找 direction-container
            direction_elem = self.driver.find_element(
                By.CSS_SELECTOR,
                ".direction-container"
            )
            return direction_elem.text.strip()
        except:
            pass

        return ""

    def parse_all(self) -> Tuple[List[Question], str]:
        """解析所有可见题目"""
        # 首先检查是否是讨论板页面
        if self._is_discussion_board_page():
            print("    🔍 检测到讨论板页面，跳过")
            return [], ""

        containers = self._find_containers()
        questions = []
        directions = self._extract_directions_from_page()

        print(f"    🔍 找到 {len(containers)} 个题目容器")

        for idx, container in enumerate(containers, 1):
            try:
                if not self._is_really_visible(container):
                    print(f"      容器 {idx} 不可见，跳过")
                    continue

                question = self._parse_single(container, idx, directions)
                if question:
                    if question.is_interactive():
                        questions.append(question)
                        print(f"      题目 {idx}: {question.q_type.name} - {question.text[:50]}...")
                    else:
                        print(f"      题目 {idx} 非交互类型: {question.q_type.name}")
                else:
                    print(f"      容器 {idx} 解析为None")

            except Exception as e:
                error_msg = str(e)
                print(f"      ⚠️ 解析容器 {idx} 失败:{error_msg[:50]}")
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
                continue

        return questions, directions

    def _is_discussion_board_page(self) -> bool:
        """检查当前页面是否是讨论板"""
        try:
            # 强特征检测
            strong_indicators = [
                '.discussion-course-page-sdk',
                '.discussion-title',
                '.ds-discussion-bottom-textArea-container',
                '.discussion-cloud-recordList'
            ]

            score = 0
            for indicator in strong_indicators:
                if self.driver.find_elements(By.CSS_SELECTOR, indicator):
                    score += 1

            # 至少满足2个条件才认为是讨论板
            if score >= 2:
                print(f"    📋 讨论板检测得分: {score}/{len(strong_indicators)}")
                return True
            return False
        except Exception as e:
            error_msg = str(e)
            print(f"操作失败: {error_msg[:50]}")  # 控制台只显示简短信息
            logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
            return False

    def _find_containers(self) -> List[Any]:
        """查找题目容器 - 优先查找独立的题目元素"""

        # 方法0：首先检查是否是讨论板页面
        if self._is_discussion_board_page():
            print("    🔍 检测到讨论板页面，跳过")
            return []

        # ====== 关键修复：优先处理阅读问答题（在选词填空之前）======
        reading_containers = self._find_reading_question_containers()
        if reading_containers:
            print(f"    🔍 找到 {len(reading_containers)} 道阅读问答题（共享材料）")
            return reading_containers

        # 方法1：查找阅读理解/选择题容器（每个.question-common-abs-reply是一道独立题目）
        choice_containers = self.driver.find_elements(
            By.CSS_SELECTOR,
            '.question-common-abs-reply > .question-common-abs-choice'
        )

        if len(choice_containers) >= 2:
            reply_containers = []
            for choice in choice_containers:
                try:
                    reply = choice.find_element(By.XPATH,
                                                './parent::div[contains(@class, "question-common-abs-reply")]')
                    if reply not in reply_containers:
                        reply_containers.append(reply)
                except:
                    pass

            if reply_containers:
                print(f"    🔍 找到 {len(reply_containers)} 道独立选择题")
                return reply_containers

        # 方法2：查找选词填空（保持原有逻辑）
        banked_containers = WebDriverHelper.safe_find_elements(
            self.driver,
            ['.layoutBody-container.has-material.has-reply']
        )
        valid_banked = []
        for container in banked_containers:
            has_options = container.find_elements(By.CSS_SELECTOR, '.option-wrapper .option')
            has_blanks = container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .comp-abs-input input')
            if has_options and has_blanks:
                valid_banked.append(container)

        if valid_banked:
            print(f"    🔍 找到 {len(valid_banked)} 个选词填空容器")
            return valid_banked

        # 方法3：查找视频容器（保持原有逻辑）
        video_containers = WebDriverHelper.safe_find_elements(
            self.driver,
            ['.layoutBody-container:has(video)', '.question-video-point-read', '.video-box']
        )
        if video_containers:
            for container in video_containers:
                has_questions = container.find_elements(By.CSS_SELECTOR,
                                                        '.question-common-abs-choice, .question-inputbox, .option, .fe-scoop')
                if not has_questions:
                    print(f"    🔍 找到纯视频容器")
                    return [container]

        # 方法4：回退到查找 layout-container（单题页面）
        containers = self.driver.find_elements(By.CSS_SELECTOR, '.layout-container')
        valid_containers = []
        for c in containers:
            try:
                has_content = (
                        c.find_elements(By.CSS_SELECTOR, '.question-inputbox, .option, .fe-scoop, textarea') or
                        c.find_elements(By.CSS_SELECTOR, 'input[type="text"]')
                )
                if has_content and c.is_displayed():
                    valid_containers.append(c)
            except:
                continue

        if valid_containers:
            print(f"    🔍 找到 {len(valid_containers)} 个有效题目容器（layout-container）")
            return valid_containers

        # 方法5：最终备用
        fallback = WebDriverHelper.safe_find_elements(
            self.driver,
            ['.layoutBody-container', '.layout-reply-container', '.reply-wrap']
        )
        if fallback:
            print(f"    🔍 备用方案找到 {len(fallback)} 个容器")
            return fallback

        return []

    def _is_really_visible(self, element) -> bool:
        """检查元素真正可见"""
        try:
            if not element.is_displayed():
                return False

            # 检查父元素是否被隐藏
            parent = element
            for _ in range(3):
                try:
                    parent = parent.find_element(By.XPATH, '..')
                    parent_display = self.driver.execute_script(
                        "return window.getComputedStyle(arguments[0]).display",
                        parent
                    )
                    if parent_display == 'none':
                        return False
                except:
                    break

            return True
        except:
            return False

    def _parse_single(self, container, number: int, directions: str = "") -> Optional[Question]:
        """使用策略解析单个容器"""
        for strategy in self.strategies:
            try:
                if strategy.can_parse(container, self.driver):
                    print(f"      使用策略: {strategy.__class__.__name__}")
                    question = strategy.parse(container, self.driver, number, directions)
                    if question:
                        if question.number is None:
                            question.number = number
                        print(f"      解析成功: {question.q_type.name}")
                        return question
                    else:
                        print(f"      策略返回None")
            except Exception as e:
                error_msg = str(e)
                print(f"      策略 {strategy.__class__.__name__} 失败: {error_msg[:50]} ")
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
                continue
        print(f"      没有匹配的策略")
        return None

    def _extract_directions(self) -> str:
        """提取题目指示"""
        directions_elem = WebDriverHelper.safe_find_element(
            self.driver,
            [".layout-direction-container", ".abs-direction", ".direction-container"]
        )
        return directions_elem.text.strip() if directions_elem else ""


class DiscussionBoardStrategy(QuestionParserStrategy):
    """讨论板策略 - 精确检测"""

    def can_parse(self, container, driver) -> bool:
        # 必须是讨论板特有的强特征
        discussion_features = [
            '.discussion-course-page-sdk',  # 最强特征
            '.ds-discussion-reply',
            '.discussion-cloud-recordList-title',  # 评论列表标题
        ]

        # 检查是否包含讨论板特征
        has_discussion_feature = any(
            container.find_elements(By.CSS_SELECTOR, feature)
            for feature in discussion_features
        )

        if not has_discussion_feature:
            return False

        # 额外检查：确保不包含选词填空特征（防止误判）
        banked_features = [
            '.question-material-banked-cloze-reply',
            '.banked-options',
            '.fe-scoop[data-scoop-index]',  # 带索引的填空
        ]

        has_banked_feature = any(
            container.find_elements(By.CSS_SELECTOR, feature)
            for feature in banked_features
        )
        # 如果同时有讨论板特征和选词填空特征，优先选词填空（更具体）
        if has_banked_feature:
            return False

        return True

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        # 返回一个标记为讨论板的题目，后续会跳过
        return Question(
            number=question_number,
            text="讨论板页面（无需作答）",
            q_type=QuestionType.DISCUSSION_BOARD,  # 需要添加这个新类型
            element=container
        )


class VocabularyTestStrategy(QuestionParserStrategy):
    """词汇测试题解析策略"""

    def can_parse(self, container, driver) -> bool:
        options = self._extract_options(container, driver)
        if len(options) < 2:
            return False

        title_elem = WebDriverHelper.safe_find_element(driver, Selectors.QUESTION_TITLE, container)
        if not title_elem:
            return False

        text = title_elem.text.strip()
        text = re.sub(r"^\d+[.、)\]]\s*", "", text)

        # 英译中 or 中译英
        is_eng_word = bool(re.match(r"^[a-zA-Z\-]+$", text)) and 1 < len(text) <= 20
        has_chinese = bool(re.search(r"[\u4e00-\u9fff]", text))

        option_texts = [opt.text for opt in options]
        has_eng_opts = any(re.search(r"[a-zA-Z]{3,}", t) for t in option_texts)
        has_chi_opts = any(re.search(r"[\u4e00-\u9fff]", t) for t in option_texts)

        return (is_eng_word and has_chi_opts) or (has_chinese and has_eng_opts)

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        title_elem = WebDriverHelper.safe_find_element(driver, Selectors.QUESTION_TITLE, container)
        text = title_elem.text.strip() if title_elem else ""

        # 修复：不再在这里添加 directions，让 PromptBuilder 统一处理
        # if directions:
        #     full_text = f"【题目要求】{directions}\n\n{text}"

        options = self._extract_options(container, driver)

        return Question(
            number=question_number,
            text=text,  # 只保留纯题目文本
            q_type=QuestionType.VOCABULARY_TEST,
            element=container,
            options=options,
            directions=directions  # 保存到 directions 字段，由 PromptBuilder 决定是否添加
        )

    def _extract_options(self, container, driver) -> List[Option]:
        """提取选项"""
        options = []
        option_elements = WebDriverHelper.safe_find_elements(driver, Selectors.CHOICE_OPTIONS, container)

        for opt_elem in option_elements:
            letter = ""
            text = ""

            # 提取选项字母
            caption_elem = WebDriverHelper.safe_find_element(driver, Selectors.OPTION_CAPTION, opt_elem)
            if caption_elem:
                letter = caption_elem.text.strip().replace('.', '').replace(')', '').replace('、', '')

            # 提取选项文本
            content_elem = WebDriverHelper.safe_find_element(driver, Selectors.OPTION_CONTENT, opt_elem)
            if content_elem:
                text = content_elem.text.strip()
            else:
                full_text = opt_elem.text.strip()
                text = re.sub(rf"^{re.escape(letter)}[.)、\\s]*", "", full_text)

            is_selected = 'selected' in (opt_elem.get_attribute('class') or '').lower()

            if letter or text:
                options.append(Option(letter=letter, text=text, element=opt_elem, is_selected=is_selected))

        return options


class BankedClozeStrategy(QuestionParserStrategy):
    """选词填空解析策略 - 修复短语支持"""

    def can_parse(self, container, driver) -> bool:
        # 检查是否有选项池（支持单词和短语）
        has_options = bool(
            container.find_elements(By.CSS_SELECTOR, '.option-wrapper .option, .option-wrapper .option-placeholder') or
            container.find_elements(By.CSS_SELECTOR, '.banked-options .option')
        )
        # 检查是否有填空位置
        has_blanks = bool(container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .comp-abs-input input'))
        return has_options and has_blanks

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        # 提取选项池（优先从 .option-placeholder 获取，支持短语）
        banked_options = []

        # 方法1：查找 .option-placeholder（短语填空用这个）
        placeholder_elements = container.find_elements(By.CSS_SELECTOR, '.option-wrapper .option-placeholder')
        for elem in placeholder_elements:
            text = elem.text.strip()
            if text and text not in banked_options:
                banked_options.append(text)

        # 方法2：如果没有找到，尝试 .option 元素
        if not banked_options:
            option_elements = container.find_elements(By.CSS_SELECTOR, '.option-wrapper .option')
            for elem in option_elements:
                text = elem.text.strip()
                if text and text not in banked_options:
                    banked_options.append(text)

        # 方法3：备选方案
        if not banked_options:
            option_elements = container.find_elements(By.CSS_SELECTOR,
                                                      '.banked-options .option, [data-rbd-draggable-id^="options-"]')
            for elem in option_elements:
                text = elem.text.strip()
                if text and text not in banked_options:
                    banked_options.append(text)

        print(f"      选项池（短语）: {banked_options}")

        # 提取所有填空位置
        inputs = []
        banked_blanks = []

        # 查找所有 scoop（填空位置）
        scoops = container.find_elements(By.CSS_SELECTOR, '.fe-scoop')

        for i, scoop in enumerate(scoops):
            # 获取上下文（整个句子）
            context = ""
            try:
                # 尝试获取父级p元素的文本
                context_elem = scoop.find_element(By.XPATH, './ancestor::p')
                context = context_elem.text.strip()
            except:
                # 如果失败，尝试获取scoop自身的文本
                try:
                    context = scoop.text.strip()
                except:
                    context = ""

            # 查找输入框
            input_box = None
            try:
                input_box = scoop.find_element(By.CSS_SELECTOR, 'input')
                inputs.append(input_box)
            except:
                pass

            banked_blanks.append({
                'index': i,
                'context': context,
                'input': input_box,
                'element': scoop
            })

        print(f"      找到 {len(banked_blanks)} 个填空位置")

        # 构建题目文本时包含 directions
        question_text = f"选词填空（{len(banked_blanks)}个空）"
        if banked_options:
            question_text += f"\n可选选项: {', '.join(banked_options[:5])}"
            if len(banked_options) > 5:
                question_text += f" 等共{len(banked_options)}个"

        return Question(
            number=question_number,
            text=question_text,
            q_type=QuestionType.BANKED_CLOZE,
            element=container,
            inputs=inputs,
            banked_options=banked_options,
            banked_blanks=banked_blanks,
            directions=directions,
        )


class StandardChoiceStrategy(QuestionParserStrategy):
    def __init__(self):
        self._material_cache: Optional[str] = None  # 添加缓存

    def can_parse(self, container, driver) -> bool:
        # 关键修复：支持两种容器结构
        # 结构1：直接是 .question-common-abs-choice
        # 结构2：container 内有 .question-common-abs-choice

        # 检查当前 container 是否就是 choice 容器
        if container.tag_name == 'div' and 'question-common-abs-choice' in (container.get_attribute('class') or ''):
            options = container.find_elements(By.CSS_SELECTOR, '.option-wrap .option, .option.isNotReview')
        else:
            # 在 container 内查找 choice 容器
            choices = container.find_elements(By.CSS_SELECTOR, '.question-common-abs-choice')
            if choices:
                # 如果找到多个，说明这个 container 包含多道题，不应该用这个策略
                if len(choices) > 1:
                    return False
                options = choices[0].find_elements(By.CSS_SELECTOR, '.option-wrap .option, .option.isNotReview')
            else:
                options = container.find_elements(By.CSS_SELECTOR, '.option-wrap .option, .option.isNotReview')

        return len(options) >= 2

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        # 每页开始时清空缓存（在 parse 开头重置）
        if question_number == 1:
            self._material_cache = None
        # 关键修复：正确处理独立的 choice 容器
        if 'question-common-abs-choice' in (container.get_attribute('class') or ''):
            choice_container = container
        else:
            choices = container.find_elements(By.CSS_SELECTOR, '.question-common-abs-choice')
            choice_container = choices[0] if choices else container

        title_elem = choice_container.find_element(By.CSS_SELECTOR, '.ques-title')
        text = title_elem.text.strip() if title_elem else ""

        # 修复：不再调用 _build_full_text 提前添加 directions
        # full_text = self._build_full_text(container, driver, text, directions)

        # 只提取纯题目文本，directions 由 PromptBuilder 统一处理
        vocab_strategy = VocabularyTestStrategy()
        options = vocab_strategy._extract_options(container, driver)

        # 判断单选还是多选
        checkboxes = container.find_elements(By.CSS_SELECTOR, 'input[type="checkbox"]')
        is_multi = (
                checkboxes or
                'multipleChoice' in (container.get_attribute('class') or '').lower() or
                '多选' in text or
                len(options) > 4
        )
        q_type = QuestionType.MULTIPLE_CHOICE if is_multi else QuestionType.SINGLE_CHOICE

        return Question(
            number=question_number,
            text=text,  # 纯题目文本
            q_type=q_type,
            element=container,
            options=options,
            directions=directions  # 保存到字段
        )

    @deprecated
    def _build_full_text(self, container, driver, question_text: str, directions: str) -> str:
        """构建完整题目文本：directions + 阅读材料 + 题目"""
        parts = []

        if directions:
            parts.append(f"【题目要求】{directions}")

        # 尝试提取阅读材料
        material = self._extract_material(container, driver)
        print(f"    [调试] 阅读材料长度: {len(material) if material else 0}")
        if material:
            parts.append(f"【阅读材料】\n{material}")

        parts.append(f"【问题】{question_text}")

        return "\n\n".join(parts)

    @deprecated
    def _extract_material(self, container, driver) -> str:
        """提取阅读材料 - 带缓存"""
        # 如果已有缓存，直接返回
        if self._material_cache is not None:
            return self._material_cache

        # 否则提取并缓存
        material = ""
        try:
            material_elem = driver.find_element(
                By.CSS_SELECTOR,
                ".layout-material-container .text-material-wrapper"
            )
            material = material_elem.text.strip()
            self._material_cache = material  # 缓存结果
        except:
            pass

        return material


class TextInputStrategy(QuestionParserStrategy):
    """文本输入题策略 - 精确区分写作题和阅读问答题"""

    # 写作题材料特征关键词
    WRITING_KEYWORDS = ['topic', 'topic sentence', 'outline', 'things to do',
                        'concluding sentence', 'more topics']
    # 阅读材料特征（长段落，包含大量文本）
    READING_MIN_LENGTH = 400  # 阅读材料通常较长

    def can_parse(self, container, driver) -> bool:
        # 基础：必须有 textarea
        textareas = container.find_elements(By.CSS_SELECTOR,
                                            'textarea.question-textarea-content, textarea.question-inputbox-input, textarea.scoopFill_textarea')
        if not textareas:
            return False

        # 获取容器类名，用于判断类型
        container_class = container.get_attribute('class') or ''

        # ========== 情况1：单个问题容器（已被拆分）==========
        is_single_reply = 'question-common-abs-reply' in container_class

        if is_single_reply:
            # 情况1a：包含inputbox -> 阅读问答题（拆分后的单题）
            has_inputbox = container.find_elements(By.CSS_SELECTOR, '.question-inputbox')
            if has_inputbox:
                return True

            # 情况1b：包含scoop结构 -> 写作题（单题）
            # 注意：写作题是 .question-common-abs-scoop 包含 .fe-scoop
            has_scoop_container = container.find_elements(By.CSS_SELECTOR, '.question-common-abs-scoop, .fe-scoop')
            if has_scoop_container:
                return True

            return False

        # ========== 情况2：大容器（未被拆分）==========
        # 检查是否包含material（写作题和阅读题都有）
        has_material = container.find_elements(By.CSS_SELECTOR, '.layout-material-container')

        if has_material:
            material_text = ""
            try:
                material = container.find_element(By.CSS_SELECTOR, '.layout-material-container')
                material_text = material.text.lower()
            except:
                pass

            # 强特征：明确包含写作提纲关键词 -> 写作题
            if any(kw in material_text for kw in self.WRITING_KEYWORDS):
                # 确认包含scoop结构（不是选词填空）
                has_scoop = container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .question-common-abs-scoop')
                if has_scoop:
                    return True

            # 强特征：包含 Model/Example -> 句子完成题
            if any(kw in material_text for kw in ['model', 'example', '示例', '例句']):
                return True

        # 检查 direction（从大容器或driver）
        direction_text = ""
        try:
            # 优先从driver获取全局direction
            direction_elem = driver.find_element(By.CSS_SELECTOR, '.layout-direction-container .component-htmlview')
            direction_text = direction_elem.text.lower()
        except:
            # 回退到容器内查找
            direction_elem = WebDriverHelper.safe_find_element(
                driver, ['.layout-direction-container .content', '.abs-direction .content'], container)
            if direction_elem:
                direction_text = direction_elem.text.lower()

        if direction_text:
            # 明确写作指令
            if any(kw in direction_text for kw in ['write', 'essay', 'composition', 'paragraph']):
                # 确认不是选词填空（有options）或选择题
                has_options = container.find_elements(By.CSS_SELECTOR, '.option-wrapper, .banked-options, .option-wrap')
                has_scoop = container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .question-common-abs-scoop')
                if not has_options and has_scoop:
                    return True

            # 明确是回答问题
            if any(kw in direction_text for kw in ['answer', 'question', 'according to']):
                # 必须有inputbox且没有scoop（否则可能是写作题）
                has_inputbox = container.find_elements(By.CSS_SELECTOR, '.question-inputbox')
                has_scoop = container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .question-common-abs-scoop')
                if has_inputbox and not has_scoop:
                    return True

        # 多个textarea且是inputbox结构 -> 阅读问答题（未拆分的大容器）
        if len(textareas) >= 2:
            has_inputbox = container.find_elements(By.CSS_SELECTOR, '.question-inputbox')
            has_scoop = container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .question-common-abs-scoop')
            # 有inputbox无scoop -> 阅读问答题
            if has_inputbox and not has_scoop:
                return True

        # 单个大textarea（5行以上）且是scoop结构 -> 写作题
        if len(textareas) == 1:
            rows = textareas[0].get_attribute('rows')
            is_scoop = container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .question-common-abs-scoop')
            if rows and int(rows) >= 5 and is_scoop:
                return True

        return False

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        try:
            effective_directions = directions

            if not effective_directions:
                try:
                    direction_elem = driver.find_element(By.CSS_SELECTOR,
                                                         ".layout-direction-container .component-htmlview")
                    effective_directions = direction_elem.text.strip()
                except:
                    pass

            # ========== 关键修复：优先根据容器结构判断题型 ==========
            container_class = container.get_attribute('class') or ''
            is_single_reply = 'question-common-abs-reply' in container_class

            # 检查容器内容特征
            has_inputbox = container.find_elements(By.CSS_SELECTOR, '.question-inputbox')
            has_scoop = container.find_elements(By.CSS_SELECTOR, '.fe-scoop, .question-common-abs-scoop')

            # 根据结构直接判断题型
            if is_single_reply:
                if has_inputbox and not has_scoop:
                    # 单个reply + inputbox = 阅读问答题
                    is_writing = False
                    print(f"      [调试] 根据结构判断为阅读问答题（单个reply+inputbox）")
                elif has_scoop and not has_inputbox:
                    # 单个reply + scoop = 写作题
                    is_writing = True
                    print(f"      [调试] 根据结构判断为写作题（单个reply+scoop）")
                else:
                    # 不确定，fallback到材料内容判断
                    is_writing = self._check_is_writing_by_material(container, driver)
            else:
                # 大容器，使用材料内容判断
                is_writing = self._check_is_writing_by_material(container, driver)

            # 提取材料文本（用于构建prompt）
            material_text = self._extract_material_text(container, driver)

            items = []

            if is_writing:
                # ========== 写作题结构（.fe-scoop） ==========
                print(f"      [调试] 处理写作题结构")
                scoop_containers = container.find_elements(By.CSS_SELECTOR, '.fe-scoop')

                for i, scoop in enumerate(scoop_containers, 1):
                    try:
                        number_elem = scoop.find_element(By.CSS_SELECTOR, '.question-number')
                        number = number_elem.text.strip()

                        textarea = scoop.find_element(By.CSS_SELECTOR, 'textarea.question-textarea-content')
                        placeholder = textarea.get_attribute('placeholder') or "写作"

                        items.append({
                            'index': i,
                            'words': f"题{number}: {placeholder}",
                            'input': textarea,
                            'question_text': placeholder
                        })
                    except Exception as e:
                        print(f"      解析第{i}个scoop失败: {str(e)[:50]}")
                        continue
            else:
                # ========== 阅读问答题结构（.question-inputbox） ==========
                print(f"      [调试] 处理阅读问答题结构")

                if is_single_reply:
                    # 单个reply容器，直接解析其中的inputbox
                    input_boxes = container.find_elements(By.CSS_SELECTOR, '.question-inputbox')
                else:
                    # 大容器，查找所有inputbox
                    input_boxes = container.find_elements(By.CSS_SELECTOR, '.question-inputbox')

                # ========== 关键修复：使用传入的 question_number 作为起始索引 ==========
                for i, box in enumerate(input_boxes, question_number):
                    try:
                        # 提取具体问题文本
                        header = box.find_element(By.CSS_SELECTOR, '.question-inputbox-header')
                        question_text = header.text.strip()
                        # 清理题号 "1. What..." -> "What..."
                        question_text = re.sub(rf'^\d+[\s.、)]+', '', question_text)

                        textarea = box.find_element(By.CSS_SELECTOR, 'textarea')

                        items.append({
                            'index': i,  # 使用全局题号
                            'words': question_text,
                            'input': textarea,
                            'question_text': question_text
                        })
                    except Exception as e:
                        print(f"      解析第{i}题失败: {str(e)[:50]}")
                        continue

            if not items:
                print(f"      未找到题目项")
                return None

            # 构建完整题目文本
            full_text = ""
            if effective_directions:
                full_text += f"【题目要求】{effective_directions}\n\n"

            if material_text:
                if is_writing:
                    full_text += f"【写作提纲/材料】\n{material_text[:500]}\n\n"
                else:
                    # 阅读题：材料是文章，需要传给AI但可能很长
                    full_text += f"【阅读材料】\n{material_text[:800]}...\n\n（文章较长，根据问题回答即可）\n\n"

            # 构建具体问题列表
            full_text += "【问题列表】\n"
            for item in items:
                if is_writing:
                    # 写作题：显示题号和提示
                    full_text += f"{item['index']}. {item['words']}\n"
                else:
                    # 阅读题：显示完整问题
                    full_text += f"{item['index']}. {item['question_text']}\n"

            print(f"      [调试] 最终题目文本长度: {len(full_text)}")
            print(f"      [调试] 题目类型: {'写作题' if is_writing else '阅读问答题'}")

            # ========== 关键修复：对于单个reply的阅读题，返回单个题目而不是合并 ==========
            if is_single_reply and not is_writing and len(items) == 1:
                # 单个阅读问答题，直接返回
                item = items[0]
                return Question(
                    number=item['index'],  # 使用正确的全局题号
                    text=full_text,
                    q_type=QuestionType.TEXT,
                    element=container,
                    inputs=[item['input']],
                    banked_blanks=[item],
                    directions=effective_directions,
                )

            # 否则返回合并的题目（写作题或大容器阅读题）
            return Question(
                number=question_number,
                text=full_text,
                q_type=QuestionType.TEXT,
                element=container,
                inputs=[item['input'] for item in items],
                banked_blanks=items,
                directions=effective_directions,
            )

        except Exception as e:
            error_msg = str(e)
            print(f"      TextInputStrategy解析失败: {error_msg[:100]}")
            logger.error(f"详细错误: {error_msg}", exc_info=True)
            return None

    def _check_is_writing_by_material(self, container=None, driver=None) -> bool:
        """通过材料内容判断是否是写作题"""
        material_text = self._extract_material_text(container, driver)

        if material_text:
            material_lower = material_text.lower()
            return any(kw in material_lower for kw in self.WRITING_KEYWORDS)

        return False  # 默认不是写作题

    def _extract_material_text(self, container=None, driver=None) -> str:
        """提取材料文本"""
        material_text = ""

        try:
            if container:
                try:
                    material = container.find_element(By.CSS_SELECTOR, '.layout-material-container')
                    material_text = material.text.strip()
                except:
                    pass

            if not material_text and driver:
                try:
                    material = driver.find_element(By.CSS_SELECTOR, '.layout-material-container')
                    material_text = material.text.strip()
                except:
                    pass
        except:
            pass

        return material_text


class ListeningFillInStrategy(QuestionParserStrategy):
    """听力填空题解析策略（仅解析题目，转录由预处理完成）"""

    def can_parse(self, container, driver) -> bool:
        # 关键特征：包含 fe-scoop 填空，且没有选项池（排除选词填空）
        has_blanks = bool(container.find_elements(By.CSS_SELECTOR, '.fe-scoop input'))
        has_option_pool = bool(container.find_elements(By.CSS_SELECTOR, '.option-wrapper, .banked-options'))
        if not has_blanks or has_option_pool:
            return False

        # 确认是听力题：direction 包含关键词
        try:
            direction = driver.find_element(By.CSS_SELECTOR, '.layout-direction-container, .abs-direction')
            text = direction.text.lower()
            return any(kw in text for kw in ['listen', 'audio', 'hear', 'talk', 'conversation'])
        except:
            return False

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        # 提取填空输入框
        inputs = container.find_elements(By.CSS_SELECTOR, '.fe-scoop input')
        if not inputs:
            return None

        # 构建简单的题目描述
        full_text = f"听力填空题（共{len(inputs)}个空）"
        if directions:
            full_text = f"【题目要求】{directions}\n\n{full_text}"

        # 获取句子上下文（可选）
        blank_contexts = []
        for i, inp in enumerate(inputs):
            try:
                scoop = inp.find_element(By.XPATH, './ancestor::span[@class="fe-scoop"]')
                sentence = scoop.find_element(By.XPATH, './ancestor::p').text
            except:
                sentence = ""
            blank_contexts.append({
                'index': i,
                'sentence': sentence,
                'input': inp
            })

        return Question(
            number=question_number,
            text=full_text,
            q_type=QuestionType.LISTENING_FILL_IN,
            element=container,
            inputs=inputs,
            banked_blanks=blank_contexts,
            directions=directions,
        )


class FillInStrategy(QuestionParserStrategy):
    """填空题解析策略 - 严格排除写作题"""

    FILL_INPUTS = [
        'input.fill-blank--bc-input-DelG1',
        '.fe-scoop input:not([type="hidden"])',  # 关键：只选 input，不选 textarea
        '.comp-abs-input input',
        '.blankinput',
        'input[type="text"]',
        # 明确排除：'textarea.question-textarea-content'
    ]

    def can_parse(self, container, driver) -> bool:
        # 关键修复1：如果有写作材料区，直接排除（让给 TextInputStrategy）
        has_material_container = container.find_elements(
            By.CSS_SELECTOR, '.layout-material-container'
        )
        if has_material_container:
            return False

        # 关键修复2：如果有大textarea，排除
        has_textarea = container.find_elements(
            By.CSS_SELECTOR, 'textarea.question-textarea-content'
        )
        if has_textarea:
            return False

        inputs = WebDriverHelper.safe_find_elements(driver, self.FILL_INPUTS, container)

        # 关键修复3：检查是否是多个小输入框（填空题特征）
        if len(inputs) >= 2:
            return True

        # 单个输入框需要进一步判断
        if len(inputs) == 1:
            # 检查placeholder或上下文是否像填空
            inp = inputs[0]
            placeholder = inp.get_attribute('placeholder') or ''
            # 写作题placeholder通常提示字数，填空题提示"请输入答案"或为空
            if 'word' in placeholder.lower() or '不少于' in placeholder:
                return False
            return True

        return False

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        title_elem = WebDriverHelper.safe_find_element(driver, Selectors.QUESTION_TITLE, container)
        text = title_elem.text.strip() if title_elem else ""

        # 清理题号
        text = re.sub(r"^\d+[.、)\]]\s*", "", text)

        # if directions:
        #     text = f"【题目要求】{directions}\n\n{text}"
        # 提取所有输入框（严格限定为input，不是textarea）
        inputs = WebDriverHelper.safe_find_elements(driver, self.FILL_INPUTS, container)

        # 按 data-scoop-index 排序，确保顺序正确
        inputs.sort(key=lambda x: int(
            x.find_element(By.XPATH, './ancestor::span[@class="fe-scoop"]').get_attribute('data-scoop-index') or 0))

        return Question(
            number=question_number,
            text=text,
            q_type=QuestionType.FILL_IN,
            element=container,
            inputs=inputs,
            directions=directions,
        )





class VocabularyFlashcardStrategy(QuestionParserStrategy):
    """单词闪卡策略 - 修复版本"""

    def can_parse(self, container, driver) -> bool:
        # 多种可能的闪卡容器特征
        flashcard_indicators = [
            '.vocContainer',
            '.vocabulary-flashcard',
            '.flashcard-container',
            '.vocActions',
            '.vocabulary-actions'
        ]

        for indicator in flashcard_indicators:
            if container.find_elements(By.CSS_SELECTOR, indicator):
                # 额外检查：不包含选择题特征
                has_choice = container.find_elements(By.CSS_SELECTOR, '.option-wrap, .question-common-abs-choice')
                if not has_choice:
                    return True

        return False

    def parse(self, container, driver, question_number: int, direction: str = "") -> Optional[Question]:
        return Question(
            number=question_number,
            text="单词闪卡",
            q_type=QuestionType.VOCABULARY_FLASHCARD,
            element=container
        )


class DropdownSelectStrategy(QuestionParserStrategy):
    """下拉选择填空题解析策略"""

    def can_parse(self, container, driver) -> bool:
        # 检查是否有下拉选择特征
        selects = container.find_elements(By.CSS_SELECTOR, '.scoop-select-wrapper, select, .ant-dropdown-trigger')
        return bool(selects)

    def parse(self, container, driver, question_number: int, directions: str = "") -> Optional[Question]:
        # 提取所有下拉选择空
        blanks = []
        select_elements = container.find_elements(By.CSS_SELECTOR, '.scoop-select-wrapper')

        for i, elem in enumerate(select_elements):
            # 获取上下文（句子）
            context = ""
            try:
                context_elem = elem.find_element(By.XPATH, './ancestor::li')
                context = context_elem.text.strip()
            except:
                pass

            # 提取选项（从隐藏的div中）
            options = []
            try:
                hidden_div = elem.find_element(By.CSS_SELECTOR, 'div[style*="visibility: hidden"]')
                option_elems = hidden_div.find_elements(By.TAG_NAME, 'i')
                for opt in option_elems:
                    text = opt.text.strip()
                    if text and text not in options:
                        options.append(text)
            except:
                # 备选：从select元素提取
                try:
                    select = elem.find_element(By.TAG_NAME, 'select')
                    option_elems = select.find_elements(By.TAG_NAME, 'option')
                    for opt in option_elems:
                        text = opt.text.strip()
                        if text and text not in ['', '点击选择']:
                            options.append(text)
                except:
                    pass

            blanks.append({
                'index': i,
                'context': context,
                'element': elem,
                'options': options
            })

        # 提取题目文本（direction）
        title_elem = WebDriverHelper.safe_find_element(driver, Selectors.QUESTION_TITLE, container)
        text = title_elem.text.strip() if title_elem else "下拉选择填空"
        if directions:
            text = directions + text
        return Question(
            number=question_number,
            text=text,
            q_type=QuestionType.DROPDOWN_SELECT,
            element=container,
            banked_blanks=blanks,
            banked_options=list(set(opt for b in blanks for opt in b['options']))  # 汇总所有选项
        )


# ==================== ai提示词构建器 ====================
class PromptBuilder:
    """Prompt构建器 - 构建AI提示词"""

    def __init__(self, kimi_client=None):  # 添加参数
        self.kimi = kimi_client  # 保存引用

    def build(self, questions: List[Question], global_directions: str = "") -> str:
        """构建Prompt - 支持题目级和全局 directions"""
        lines = []

        # 说明有多篇材料
        if len(self.kimi.accumulated_passages) > 1:
            lines.append(f"【注意】本章节共有 {len(self.kimi.accumulated_passages)} 篇阅读材料，请根据问题判断使用哪篇。")
            lines.append("")

        # 关键修复：只添加一次 directions，优先使用 global_directions
        effective_directions = global_directions
        if not effective_directions and questions:
            effective_directions = questions[0].directions  # 回退到题目自带的 directions

        if effective_directions:
            lines.append(f"【题目指示】{effective_directions}")
            lines.append("")  # 空行分隔

        # 检测题型组合
        type_counts = {}
        for q in questions:
            type_counts[q.q_type] = type_counts.get(q.q_type, 0) + 1

        # 添加全局提示
        if QuestionType.VOCABULARY_TEST in type_counts:
            lines.extend(self._vocabulary_test_hints())

        if QuestionType.BANKED_CLOZE in type_counts:
            lines.extend(self._banked_cloze_hints())

        # 构建每道题
        for q in questions:
            builder_method = self._get_builder_method(q.q_type)
            lines.extend(builder_method(q))

        # 添加格式说明
        lines.extend(self._format_instructions(type_counts))

        return '\n'.join(lines)

    def _vocabulary_test_hints(self) -> List[str]:
        """词汇测试提示"""
        return [
            "【重要提示】这是词汇测试题，包含以下类型：",
            "- 类型A（英文→中文）：题干是英文单词，选项是中文释义",
            "- 类型B（中文→英文）：题干是中文释义，选项是英文单词",
            "- 类型C（语境填空）：题干是英文句子，选项是单词填入",
            "请仔细分析每道题的具体类型，选择最准确的答案。\n"
        ]

    def _banked_cloze_hints(self) -> List[str]:
        """选词填空提示"""
        return [
            "【重要提示】这是选词填空题，请从给定的单词列表中选择最合适的填入空白处。\n",
            "【格式要求】只需返回答案本身，不要添加括号注释、不要解释、不要变形说明！"
        ]

    def _get_builder_method(self, q_type: QuestionType) -> Callable[[Question], List[str]]:
        """获取对应题型的构建方法"""
        builders = {
            QuestionType.VOCABULARY_TEST: self._build_vocab_test,
            QuestionType.BANKED_CLOZE: self._build_banked_cloze,
            QuestionType.DROPDOWN_SELECT: self._build_dropdown_select,
            QuestionType.SINGLE_CHOICE: self._build_single_choice,
            QuestionType.MULTIPLE_CHOICE: self._build_multiple_choice,
            QuestionType.FILL_IN: self._build_fill_in,
            QuestionType.TEXT: self._build_text,

            QuestionType.VOCABULARY_FLASHCARD: lambda q: [],  # 闪卡无题目
            QuestionType.LISTENING_FILL_IN: self._build_listening_fill_in,
        }
        return builders.get(q_type, self._build_unknown)

    def _build_listening_fill_in(self, q: Question) -> List[str]:
        """构建听力填空题Prompt"""
        lines = [
            f"{q.number}. 【听力填空题】",
            f"{q.text}",
            "",
            "【答题要求】",
            "1. 这是一个听力理解题，请根据句子上下文和逻辑填写最合适的单词或短语",
            "2. 每个空填写一个简洁的答案（单词或短句）",
            "3. 注意语法正确性和上下文连贯性",
            ""
        ]

        for blank in q.banked_blanks:
            lines.append(f"   空{blank['index'] + 1}: {blank['sentence']}")

        lines.append("")
        return lines

    def _build_vocab_test(self, q: Question) -> List[str]:
        """构建词汇测试题"""
        lines = [f"{q.number}. 【词汇题】{q.text}"]

        # 分析子类型
        text_clean = re.sub(r"^\d+[.、)\]]\s*", "", q.text).strip()

        if bool(re.match(r"^[a-zA-Z\-]+$", text_clean)) and len(text_clean) <= 20:
            lines.append("   → 选择该英文单词的正确中文释义")
        elif bool(re.search(r"[\u4e00-\u9fff]", text_clean)):
            lines.append("   → 选择该中文释义对应的正确英文表达")
        elif "_" in text_clean or len(text_clean) > 50:
            lines.append("   → 根据句子语境选择最合适的单词")

        for opt in q.options:
            lines.append(f"   {opt.letter}. {opt.text}")
        lines.append("")
        return lines

    def _build_banked_cloze(self, q: Question) -> List[str]:
        """构建选词填空题（支持单词和短语）"""

        # 检测是单词还是短语
        is_phrase = q.is_phrase_mode

        lines = [
            f"{q.number}. 【选词填空】请从以下选项中选择最合适的{'短语' if is_phrase else '单词'}填入空白处：",
            f"   可选{'短语' if is_phrase else '单词'}: {', '.join(q.banked_options)}",
        ]

        if is_phrase:
            lines.extend([
                "注意：这是短语填空！请填写完整短语（如 'in advance' 而不是 'advance'）。",
                "必要时需要改变短语的形式（如时态、单复数等）。",
            ])
        else:
            lines.extend([
                " 注意：必要时需要改变单词形式（如时态、单复数等）。",
            ])

        lines.append("")

        for i, blank in enumerate(q.banked_blanks, 1):
            context = blank['context'][:250] + "..." if len(blank['context']) > 250 else blank['context']
            # 清理上下文中的HTML标签
            context = re.sub(r'<[^>]+>', '', context)
            lines.append(f"   空{i}: {context}")

        lines.append("")
        lines.append("   要求：")

        if is_phrase:
            lines.append("1. 必须填写完整短语（不要只填部分）")
            lines.append("2. 按顺序给出答案，格式：1.in advance 2.make the most of ...")
        else:
            lines.append("1. 按顺序给出答案，格式：1.word1 2.word2 ...")

        lines.append("")
        return lines

    def _build_single_choice(self, q: Question) -> List[str]:
        """构建单选题"""
        lines = [f"{q.number}. 【单选】{q.text}"]
        for opt in q.options:
            lines.append(f" {opt.letter}. {opt.text}")
        lines.append("")
        return lines

    def _build_multiple_choice(self, q: Question) -> List[str]:
        """构建多选题"""
        lines = [f"{q.number}. 【多选】{q.text}", "   （注意：本题有多个正确答案）"]
        for opt in q.options:
            lines.append(f"   {opt.letter}. {opt.text}")
        lines.append("")
        return lines

    def _build_fill_in(self, q: Question) -> List[str]:
        """构建填空题"""
        lines = [f"{q.number}. 【填空】{q.text}"]
        if len(q.inputs) > 1:
            lines.append(f"   （共 {len(q.inputs)} 个空）")
        lines.append("")
        return lines

    def _build_text(self, q: Question) -> List[str]:
        """构建文本题"""
        lines = [f"{q.number}. 【简答题】{q.text}"]
        if len(q.inputs) > 1:
            lines.append(f"   （共 {len(q.inputs)} 小题）")
        lines.append("   （请提供简洁准确的回答，如果不是翻译题，那么只用英文回答）")
        lines.append("")
        return lines

    def _build_unknown(self, q: Question) -> List[str]:
        """构建未知类型题"""
        lines = [f"{q.number}. 【题】{q.text}"]
        for opt in q.options:
            lines.append(f"   {opt.letter}. {opt.text}")
        lines.append("")
        return lines

    def _format_instructions(self, type_counts: Dict[QuestionType, int]) -> List[str]:
        """构建格式说明 - 修复版本"""
        lines = ["-" * 50, "请按以下格式回答："]

        has_single = QuestionType.SINGLE_CHOICE in type_counts or QuestionType.VOCABULARY_TEST in type_counts
        has_multiple = QuestionType.MULTIPLE_CHOICE in type_counts

        # 只有单选题，没有多选题
        if has_single and not has_multiple:
            lines.append("单选题: 直接返回选项字母，如：A 或 1.A")
            lines.append("注意：每道题只选一个答案！")

        # 只有多选题，没有单选题
        elif has_multiple and not has_single:
            lines.append("多选题: 返回多个字母，如：AB 或 1.AB")
            lines.append("注意：每道题可能有一个或多个正确答案！")

        # 混合题型（同时有单选和多选）
        elif has_single and has_multiple:
            lines.append("混合题型：")
            lines.append("- 单选题: 返回单个字母，如：A")
            lines.append("- 多选题: 返回多个字母，如：AB")
            lines.append("请仔细判断每道题是单选还是多选！")
            lines.append("判断依据：题目明确标注'多选'或有多个正确选项时选多个，否则单选")

        if QuestionType.BANKED_CLOZE in type_counts or QuestionType.DROPDOWN_SELECT in type_counts:
            lines.append("选词/选择填空: 1.word1 2.word2 ...")

        if QuestionType.FILL_IN in type_counts:
            lines.append("填空题: 1.答案1 2.答案2 ...")

        if QuestionType.TEXT in type_counts:
            lines.append("简答题: 1.答案内容...")

        lines.append("-" * 50)
        return lines

    def _build_dropdown_select(self, q: Question) -> List[str]:
        """构建下拉选择题"""
        lines = [
            f"{q.number}. 【选择填空】请从选项中选择合适的词填入空白处：",
            f"   可选选项: {', '.join(q.banked_options)}",
            ""
        ]

        for i, blank in enumerate(q.banked_blanks, 1):
            context = blank['context'][:200] + "..." if len(blank['context']) > 200 else blank['context']
            context = re.sub(r'<[^>]+>', '', context)
            lines.append(f"   空{i}: {context}")

        lines.append("")
        lines.append("要求：按顺序给出答案，格式：1.do 2.make ...")
        lines.append("")
        return lines


# ==================== 答案执行器 ====================
class AnswerExecutor:
    """答案执行器 - 执行答案填写"""

    def __init__(self, driver):
        self.driver = driver

    def execute(self, question: Question, answer: str) -> AnswerResult:
        """根据题型执行填写"""
        executors = {
            QuestionType.SINGLE_CHOICE: self._fill_single_choice,
            QuestionType.VOCABULARY_TEST: self._fill_single_choice,  # 同单选
            QuestionType.MULTIPLE_CHOICE: self._fill_multiple_choice,
            QuestionType.BANKED_CLOZE: self._fill_banked_cloze,
            QuestionType.DROPDOWN_SELECT: self._fill_dropdown_select,
            QuestionType.FILL_IN: self._fill_fill_in,
            QuestionType.TEXT: self._fill_text,
            QuestionType.LISTENING_FILL_IN: self._fill_listening_fill_in,
        }

        executor = executors.get(question.q_type, self._fill_unknown)
        return executor(question, answer)

    def _fill_single_choice(self, q: Question, answer: str) -> AnswerResult:
        """填写单选题 - 修复版本"""
        answer_letter = self._extract_letter(answer)
        if not answer_letter:
            return AnswerResult(False, q.number, answer, "无法解析答案")

        print(f"\t寻找选项: {answer_letter}")
        print(f"\t可用选项: {[opt.letter for opt in q.options]}")

        # 先尝试精确匹配
        for opt in q.options:
            if opt.letter.upper() == answer_letter.upper():
                print(f"\t点击选项 {opt.letter}: {opt.text[:30]}...")
                success = WebDriverHelper.safe_click(self.driver, opt.element)
                if success:
                    return AnswerResult(True, q.number, answer_letter, f"选择成功: {opt.text[:30]}")
                else:
                    return AnswerResult(False, q.number, answer, "点击失败")

        # 如果没找到，尝试模糊匹配（A-D对应索引0-3）
        try:
            idx = ord(answer_letter.upper()) - ord('A')
            if 0 <= idx < len(q.options):
                opt = q.options[idx]
                print(f"\t通过索引匹配选项 {opt.letter}: {opt.text[:30]}...")
                success = WebDriverHelper.safe_click(self.driver, opt.element)
                if success:
                    return AnswerResult(True, q.number, answer_letter, f"选择成功: {opt.text[:30]}")
        except:
            pass

        return AnswerResult(False, q.number, answer, f"未找到选项 {answer_letter}")

    def _fill_multiple_choice(self, q: Question, answer: str) -> AnswerResult:
        """填写多选题"""
        letters = re.findall(r'[A-Z]', answer.upper())
        selected = []

        for letter in letters:
            for opt in q.options:
                if opt.letter.upper() == letter and not opt.is_selected:
                    if WebDriverHelper.safe_click(self.driver, opt.element):
                        selected.append(letter)
                    break

        return AnswerResult(
            bool(selected), q.number, ','.join(selected),
            f"选中 {len(selected)}/{len(letters)} 个选项"
        )

    def _fill_banked_cloze(self, q: Question, answer: str) -> AnswerResult:
        """填写选词填空 - 支持单词和短语"""
        # 解析答案 - 关键修复：支持带空格的短语
        words = self._parse_banked_answer(answer, len(q.banked_blanks))

        # 检测模式
        is_phrase_mode = q.is_phrase_mode

        print(f"\t解析答案: {words}")
        print(f"\t填空数量: {len(q.banked_blanks)}")
        print(f"\t模式: {'短语' if is_phrase_mode else '单词'}")

        success_count = 0

        for i, (blank, word) in enumerate(zip(q.banked_blanks, words)):
            if blank['input'] and word:
                try:
                    # 清理答案
                    clean_word = word.strip()

                    # 尝试匹配到选项池中的原始形式（关键：保留完整短语）
                    matched = self._match_to_option(clean_word, q.banked_options, is_phrase_mode)
                    if matched:
                        clean_word = matched
                        print(f"        匹配到选项: {clean_word}")

                    # 滚动到可视区域
                    self.driver.execute_script(
                        "arguments[0].scrollIntoView({block: 'center', behavior: 'smooth'});",
                        blank['input']
                    )
                    time.sleep(0.3)

                    # 清空并填写
                    blank['input'].clear()
                    time.sleep(0.1)
                    blank['input'].send_keys(clean_word)

                    # 触发事件
                    self.driver.execute_script("""
                        arguments[0].dispatchEvent(new Event('input', {bubbles: true}));
                        arguments[0].dispatchEvent(new Event('change', {bubbles: true}));
                        arguments[0].dispatchEvent(new Event('blur', {bubbles: true}));
                    """, blank['input'])

                    print(f"        空{i + 1}: {clean_word}")
                    success_count += 1

                except Exception as e:
                    error_msg = str(e)
                    print(f"      填空 {i + 1} 失败:{error_msg[:50]} ")
                    logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件

        return AnswerResult(
            success_count > 0, q.number, answer,
            f"填写 {success_count}/{len(q.banked_blanks)} 个空"
        )

    def _match_to_option(self, answer: str, options: List[str], is_phrase_mode: bool) -> Optional[str]:
        """将答案匹配到选项池中的原始形式"""
        if not answer or not options:
            return None

        answer_lower = answer.lower().strip()

        # 1. 直接匹配（不区分大小写）
        for opt in options:
            if opt.lower() == answer_lower:
                return opt

        # 2. 短语模式：部分匹配（如 "advance" -> "in advance"）
        if is_phrase_mode:
            for opt in options:
                opt_lower = opt.lower()
                # 答案包含在选项中，且长度合理
                if answer_lower in opt_lower and len(answer_lower) >= 4:
                    return opt
                # 选项包含在答案中（答案可能是变形后的完整短语）
                if opt_lower in answer_lower:
                    return opt

        # 3. 单词模式：处理变形
        else:
            # 简单的变形匹配
            for opt in options:
                opt_lower = opt.lower()
                # 完全匹配
                if opt_lower == answer_lower:
                    return opt
                # 去除常见后缀匹配
                if answer_lower.rstrip('s') == opt_lower or \
                        answer_lower.rstrip('es') == opt_lower or \
                        answer_lower.rstrip('ed') == opt_lower or \
                        answer_lower.rstrip('ing') == opt_lower:
                    return opt
                # 反向匹配
                if opt_lower.rstrip('s') == answer_lower or \
                        opt_lower.rstrip('es') == answer_lower or \
                        opt_lower.rstrip('ed') == answer_lower:
                    return opt

        return None

    def _fill_fill_in(self, q: Question, answer: str) -> AnswerResult:
        """填写填空题"""
        # 关键：使用能解析 "1.xxx 2.yyy" 格式的解析器
        answers = self._parse_banked_answer(answer, len(q.inputs))
        print(f"\t解析答案: {answers}")
        print(f"\t输入框数量: {len(q.inputs)}")

        success_count = 0
        for i, inp in enumerate(q.inputs):
            ans = answers[i] if i < len(answers) else ""
            if ans:
                print(f"\t空{i + 1}: {ans}")
                WebDriverHelper.simulate_typing(self.driver, inp, ans)
                success_count += 1
            else:
                print(f"\t空{i + 1}: (空)")

        return AnswerResult(
            success_count > 0,
            q.number,
            answer,
            f"填写 {success_count}/{len(q.inputs)} 个空"
        )

    def _extract_answer_by_number(self, answer: str, question_number: int) -> str:
        """从AI回答中提取指定题号的答案"""
        # 匹配 "1. xxx" 或 "1) xxx" 等格式，直到下一个题号或结束
        pattern = rf'{question_number}\s*[.、\)\]]\s*(.+?)(?=\s*\d+\s*[.、\)\]]|$)'
        match = re.search(pattern, answer, re.DOTALL)
        if match:
            return match.group(1).strip()

        # 如果没有匹配到，尝试按行分割
        lines = [l.strip() for l in answer.split('\n') if l.strip()]
        for line in lines:
            # 去掉行首的数字
            clean = re.sub(r'^\d+\s*[.、)\]]\s*', '', line).strip()
            # 如果清理后的内容不以数字开头，可能是答案
            if clean and not re.match(r'^\d', clean):
                # 简单启发式：找到包含该题号的那一行
                if line.startswith(
                        str(question_number)) or f"{question_number}." in line or f"{question_number})" in line:
                    return clean

        return ""

    def _fill_text(self, q: Question, answer: str) -> AnswerResult:
        """填写文本题 - 修复版本：支持多小题分别填写"""
        if not q.inputs:
            return AnswerResult(False, q.number, answer, "无输入框")

        # 关键修复：根据题号从AI回答中提取对应答案
        # AI回答格式：1. xxx 2. xxx 3. xxx ...
        expected_count = len(q.inputs)

        # 如果只有一个input，尝试按题号提取
        if expected_count == 1:
            # 尝试提取该题号对应的答案
            ans = self._extract_answer_by_number(answer, q.number)
            if not ans:
                # 如果提取失败，使用原来的解析方法
                answers = self._parse_banked_answer(answer, expected_count)
                ans = answers[0] if answers else ""
        else:
            # 多个input，使用原来的解析方法
            answers = self._parse_banked_answer(answer, expected_count)
            ans = answers[0] if answers else ""

        print(f"\t题{q.number}: {ans[:60]}..." if ans else f"\t题{q.number}: (空)")

        if ans:
            inp = q.inputs[0]
            # 滚动到可视区域
            self.driver.execute_script(
                "arguments[0].scrollIntoView({block: 'center', behavior: 'smooth'});",
                inp
            )
            time.sleep(0.2)
            # 填入答案
            WebDriverHelper.simulate_typing(self.driver, inp, ans)
            return AnswerResult(True, q.number, ans, f"填写题{q.number}成功")

        return AnswerResult(False, q.number, answer, f"题{q.number}无答案")

    def _fill_unknown(self, q: Question, answer: str) -> AnswerResult:
        """未知类型"""
        return AnswerResult(False, q.number, answer, "未知题型，无法填写")

    def submit(self) -> bool:
        """提交答案 - 优化版本"""
        # 优先尝试最可能的选择器，减少等待时间
        priority_selectors = [
            '.submit-bar-pc--btn-1_Xvo',  # 最常见的提交按钮
            'button[type="submit"]',
            'button.submit-btn',
        ]
        for selector in priority_selectors:
            try:
                btn = WebDriverWait(self.driver, 2).until(  # 减少超时到2秒
                    EC.element_to_be_clickable((By.CSS_SELECTOR, selector))
                )
                if btn:
                    return WebDriverHelper.safe_click(self.driver, btn)
            except:
                continue
        # 备选：尝试其他选择器
        btn = WebDriverHelper.safe_find_element(self.driver, Selectors.SUBMIT_BUTTON, timeout=3)
        if btn:
            return WebDriverHelper.safe_click(self.driver, btn)
        return False

    @staticmethod
    def _extract_letter(answer: str) -> Optional[str]:
        """从答案中提取字母"""
        match = re.search(r'[A-D]', answer.upper())
        return match.group() if match else None

    @staticmethod
    def _parse_banked_answer(answer: str, expected_count: int) -> List[str]:
        """解析答案 - 增强版本"""
        result = [''] * expected_count

        # 清理前缀
        answer = re.sub(r'^(简答题|选词/选择填空|填空题|答案|选词填空|翻译)[：:]\s*', '', answer.strip())

        print(f"    [调试] 清理后答案前200字: {answer[:200]}...")

        # 方法1: 匹配 "数字.答案" 格式（支持多行）
        pattern = r'\d+\s*[.、\)\]]\s*(.+?)(?=\s*\d+\s*[.、\)\]]|$)'
        matches = re.findall(pattern, answer, re.DOTALL)

        print(f"    [调试] 正则表达式匹配结果数: {len(matches)}")

        if matches and len(matches) >= expected_count:
            for i, content in enumerate(matches[:expected_count]):
                clean = content.strip().replace('\n', ' ')
                result[i] = clean
            return result

        # 方法2: 如果匹配数不够，尝试更宽松的模式（按行分割）
        lines = [line.strip() for line in answer.split('\n') if line.strip()]
        content_lines = []

        for line in lines:
            # 去掉行首数字
            clean = re.sub(r'^\d+\s*[.、)\]]\s*', '', line).strip()
            if clean and not re.match(r'^\d+$', clean):  # 排除只有数字的行
                content_lines.append(clean)

        print(f"    [调试] 按行分割结果数: {len(content_lines)}")

        for i, content in enumerate(content_lines[:expected_count]):
            result[i] = content

        return result

    def _fill_dropdown_select(self, q: Question, answer: str) -> AnswerResult:
        """填写下拉选择题 - 修复React Ant Design下拉组件"""

        answers = self._parse_banked_answer(answer, len(q.banked_blanks))
        print(f"      解析答案: {answers}")
        print(f"      填空数量: {len(q.banked_blanks)}")

        success_count = 0

        for i, (blank, ans) in enumerate(zip(q.banked_blanks, answers)):
            if not ans:
                continue

            try:
                print(f"      空{i + 1}: '{ans}'")

                # 获取scoop-select-wrapper元素
                select_wrapper = blank['element']

                # 滚动到可视区域
                self.driver.execute_script(
                    "arguments[0].scrollIntoView({block: 'center', behavior: 'smooth'});",
                    select_wrapper
                )
                time.sleep(0.5)

                # ========== 关键修复1：使用Ant Design的标准交互流程 ==========

                # 1. 点击触发器打开下拉菜单（必须使用真实点击触发React事件）
                trigger = select_wrapper.find_element(By.CSS_SELECTOR, '.ant-dropdown-trigger')

                # 使用ActionChains模拟真实用户行为序列
                actions = ActionChains(self.driver)
                actions.move_to_element(trigger).click().perform()
                print(f"        ✓ 点击触发器打开下拉")
                time.sleep(0.8)  # 等待下拉菜单动画和DOM插入

                # 2. 等待下拉菜单出现（动态插入到body或特定容器）
                dropdown_menu = None
                for attempt in range(5):
                    try:
                        # Ant Design下拉菜单通常挂载到body或特定定位容器
                        dropdown_menu = WebDriverWait(self.driver, 2).until(
                            EC.presence_of_element_located((
                                By.CSS_SELECTOR,
                                '.ant-dropdown:not(.ant-dropdown-hidden) .ant-dropdown-menu, '
                                '.ant-select-dropdown:not(.ant-select-dropdown-hidden) .ant-select-item'
                            ))
                        )
                        if dropdown_menu.is_displayed():
                            break
                    except:
                        pass
                    time.sleep(0.3)

                if not dropdown_menu:
                    print(f"        ⚠️ 下拉菜单未出现，尝试备选方案")
                    # 备选：直接通过JavaScript触发选择
                    if self._force_select_by_js(select_wrapper, ans):
                        success_count += 1
                    continue

                # 3. 查找并点击选项（支持多种可能的选择器）
                option_selectors = [
                    f'.ant-dropdown-menu-item:contains("{ans}")',
                    f'.ant-select-item-option:contains("{ans}")',
                    f'.ant-dropdown-menu-item[title="{ans}"]',
                    '//li[contains(@class,"ant-dropdown-menu-item") and contains(text(),"{}")]'.format(ans),
                    '//div[contains(@class,"ant-select-item-option-content") and contains(text(),"{}")]'.format(ans)
                ]

                option_clicked = False

                # 尝试CSS选择器
                for selector in option_selectors[:3]:
                    try:
                        options = self.driver.find_elements(By.CSS_SELECTOR,
                                                            selector.replace(f':contains("{ans}")', ''))
                        for opt in options:
                            if ans.lower() in opt.text.lower() and opt.is_displayed():
                                # 使用ActionChains点击确保事件触发
                                ActionChains(self.driver).move_to_element(opt).click().perform()
                                print(f"        ✓ 点击选项: {opt.text[:20]}")
                                option_clicked = True
                                break
                        if option_clicked:
                            break
                    except Exception as e:
                        continue

                # 尝试XPath
                if not option_clicked:
                    for xpath in option_selectors[3:]:
                        try:
                            option = self.driver.find_element(By.XPATH, xpath)
                            if option.is_displayed():
                                ActionChains(self.driver).move_to_element(option).click().perform()
                                print(f"        ✓ XPath点击选项")
                                option_clicked = True
                                break
                        except:
                            continue

                # 4. 验证选择是否成功（关键步骤）
                if option_clicked:
                    time.sleep(0.5)  # 等待React状态更新

                    # 检查视觉反馈：user-answer-text应该更新，empty类应该移除
                    try:
                        answer_text_elem = select_wrapper.find_element(By.CSS_SELECTOR, '.user-answer-text')
                        displayed_text = answer_text_elem.text.strip()

                        # 同时检查trigger的class是否包含selected或不含empty
                        trigger_class = trigger.get_attribute('class') or ''

                        if ans.lower() in displayed_text.lower() or 'empty' not in trigger_class:
                            print(f"        ✓ 验证成功，显示文本: {displayed_text[:20]}")
                            success_count += 1
                        else:
                            print(f"        ⚠️ 视觉反馈异常，文本: {displayed_text[:20]}")
                            # 尝试强制同步React状态
                            self._sync_react_state(select_wrapper, ans)

                    except Exception as e:
                        print(f"        ⚠️ 验证失败: {str(e)[:50]}")
                        success_count += 1  # 保守认为成功，因为点击已执行

                else:
                    print(f"        ❌ 未找到选项 '{ans}'")
                    # 尝试强制方案
                    if self._force_select_by_js(select_wrapper, ans):
                        success_count += 1

            except Exception as e:
                print(f"        处理空{i + 1}失败: {str(e)[:50]}")
                logger.error(f"详细错误: {str(e)}", exc_info=True)
                continue

        return AnswerResult(
            success_count > 0,
            q.number,
            answer,
            f"成功 {success_count}/{len(q.banked_blanks)} 个"
        )

    def _fill_listening_fill_in(self, q: Question, answer: str) -> AnswerResult:
        """填写听力填空 - 与普通填空类似，但使用句子上下文解析"""
        # 使用banked_cloze的解析逻辑（支持 "1.xxx 2.xxx" 格式）
        answers = self._parse_banked_answer(answer, len(q.inputs))

        print(f"\t解析答案: {answers}")
        print(f"\t输入框数量: {len(q.inputs)}")

        success_count = 0
        for i, (blank_info, ans) in enumerate(zip(q.banked_blanks, answers)):
            if ans and blank_info['input']:
                print(f"\t空{i + 1}: {ans}")
                # 使用句子上下文验证答案合理性（可选）
                WebDriverHelper.simulate_typing(self.driver, blank_info['input'], ans)
                success_count += 1

        return AnswerResult(
            success_count > 0,
            q.number,
            answer,
            f"填写 {success_count}/{len(q.inputs)} 个空"
        )

    def _force_select_by_js(self, select_wrapper, value: str) -> bool:
        """强制通过JavaScript设置值（备选方案）"""
        try:
            js = """
            // 模拟完整的React事件序列
            var wrapper = arguments[0];
            var value = arguments[1];

            // 1. 找到隐藏的input或触发器
            var trigger = wrapper.querySelector('.ant-dropdown-trigger');

            // 2. 创建并分发完整的事件序列
            var events = ['mousedown', 'focus', 'click', 'input', 'change', 'blur'];

            events.forEach(function(eventType) {
                var event = new Event(eventType, { bubbles: true, cancelable: true });
                trigger.dispatchEvent(event);
            });

            // 3. 更新视觉显示
            var textElem = wrapper.querySelector('.user-answer-text');
            if (textElem) {
                textElem.innerHTML = '<p>' + value + '</p>';
                textElem.textContent = value;
            }

            // 4. 移除empty类，添加selected类
            trigger.classList.remove('empty');
            trigger.classList.add('selected');

            // 5. 尝试找到React实例并强制更新（如果存在）
            var reactKey = Object.keys(trigger).find(k => k.startsWith('__react'));
            if (reactKey) {
                var fiber = trigger[reactKey];
                // 向上查找带有props的组件
                while (fiber) {
                   if (fiber.memoizedProps && fiber.memoizedProps.onChange) {
                    fiber.memoizedProps.onChange(value);
                    return 'react_onChange_triggered';
                }
                fiber = fiber.return || fiber._debugOwner;
            }
        }

        // 6. 触发表单验证（如果存在）
        var formEvent = new Event('submit', { bubbles: true });
        var form = trigger.closest('form');
        if (form) form.dispatchEvent(formEvent);

        return 'dom_updated';
        """

            result = self.driver.execute_script(js, select_wrapper, value)
            print(f"        JS强制设置结果: {result}")

            # 验证
            time.sleep(0.3)
            text_elem = select_wrapper.find_element(By.CSS_SELECTOR, '.user-answer-text')
            return value.lower() in text_elem.text.lower()

        except Exception as e:
            print(f"        JS强制设置失败: {str(e)[:50]}")
            return False

    def _sync_react_state(self, select_wrapper, value: str) -> bool:
        """同步React状态确保表单提交时能获取值"""
        try:
            js = """
            var wrapper = arguments[0];
            var value = arguments[1];

            // 方法1：尝试通过ref或data属性存储值
            wrapper.setAttribute('data-selected-value', value);

            // 方法2：查找可能存在的隐藏input
            var hiddenInput = wrapper.querySelector('input[type="hidden"]');
            if (hiddenInput) {
                hiddenInput.value = value;
                hiddenInput.dispatchEvent(new Event('change', { bubbles: true }));
            }

            // 方法3：在window对象上存储（某些实现会读取）
            if (!window.__formData) window.__formData = {};
            var scoopIndex = wrapper.closest('[data-scoop-index]')?.getAttribute('data-scoop-index');
            if (scoopIndex) {
                window.__formData[scoopIndex] = value;
            }

            return true;
            """
            return self.driver.execute_script(js, select_wrapper, value)
        except:
            return False


# ==================== 内容处理器 ====================
class ContentHandler(ABC):
    """内容处理器基类"""

    @abstractmethod
    def can_handle(self, question: Question) -> bool:
        pass

    @abstractmethod
    def handle(self, question: Question) -> bool:
        pass


class DiscussionBoardHandler(ContentHandler):
    """讨论板处理器 - 直接跳过"""

    def __init__(self, driver):
        self.driver = driver

    def can_handle(self, question: Question) -> bool:
        return question.q_type == QuestionType.DISCUSSION_BOARD

    def handle(self, question: Question) -> bool:
        print("    💬 讨论板页面，无需作答")
        return True


class VideoHandler:
    """视频处理器 - 自动播放并智能回答弹窗问题"""

    def __init__(self, driver, config: Config):
        self.driver = driver
        self.config = config
        self.popup_monitor_thread = None
        self.stop_monitoring = threading.Event()

        # 初始化语音识别器（优先使用本地模型，避免API费用）
        self.transcriber = AudioTranscriber(
            api_key=config.whisper_api,
            use_local=True  # 默认使用本地模型，更稳定
        )

        # 轻量级AI客户端用于分析弹窗答案（不污染主Kimi的历史记录）
        self.analyzer_client = OpenAI(
            api_key=config.api_key,
            base_url=config.base_url
        )

        self.video_transcript = ""  # 存储视频转录文本
        self.current_video_url = ""  # 当前视频URL，用于缓存判断

    def _play_video_and_handle_popups(self):
        """播放视频并自动处理弹窗选择题（供外部预处理调用）"""
        video_info = self._get_video_info()
        if not video_info:
            print("      ⚠️ 未找到视频元素")
            return

        video_url = video_info.get('url', '')
        duration = video_info.get('duration', 0)

        # 转录视频（如果同一视频已缓存则跳过）
        if video_url and video_url == self.current_video_url and self.video_transcript:
            print(f"      📦 使用已缓存的视频转录（{len(self.video_transcript)}字符）")
        else:
            self.current_video_url = video_url
            self.video_transcript = self._transcribe_video(video_url, duration)

        # 启动弹窗监视线程（处理播放过程中的选择题）
        self.stop_monitoring.clear()
        self.popup_monitor_thread = threading.Thread(
            target=self._monitor_popup_questions,
            daemon=True
        )
        self.popup_monitor_thread.start()

        # 播放视频
        self._play_video(duration)

        print("      ✅ 视频播放完成")
        self.stop_monitoring.set()
        if self.popup_monitor_thread.is_alive():
            self.popup_monitor_thread.join(timeout=5)


    def _get_video_info(self) -> Optional[Dict]:
        """获取视频信息"""
        try:
            video = self.driver.find_element(By.TAG_NAME, 'video')
            url = video.get_attribute('src') or ''

            # 如果没有src，尝试source标签
            if not url:
                sources = video.find_elements(By.TAG_NAME, 'source')
                for source in sources:
                    url = source.get_attribute('src')
                    if url:
                        break

            duration = self.driver.execute_script("return arguments[0].duration;", video)

            return {
                'url': url,
                'duration': duration or 0,
                'element': video
            }
        except:
            return None

    def _transcribe_video(self, video_url: str, duration: float) -> str:
        """转录视频音频"""
        if not video_url:
            return ""

        print(f"    🎙️ 开始识别视频音频（时长: {int(duration)}秒）...")

        try:
            # 根据时长选择识别方式
            if duration > 120:  # 超过2分钟分段处理
                transcript = self.transcriber.transcribe_long_audio(
                    video_url,
                    language="en",
                    chunk_length=30
                )
            else:
                transcript = self.transcriber.transcribe(
                    video_url,
                    language="en"
                )

            if transcript:
                print(f"    ✅ 识别成功: {transcript}")
                return transcript
            else:
                print("    ⚠️ 未能识别音频内容")
                return ""

        except Exception as e:
            print(f"    ❌ 音频识别失败: {str(e)[:50]}")
            return ""

    def _play_video(self, duration: float):
        """播放视频"""
        try:
            video = self.driver.find_element(By.TAG_NAME, 'video')

            if duration > 0:
                print(f"      ▶️ 播放视频（{int(duration)}秒，2倍速）...")
                self.driver.execute_script("""
                    arguments[0].playbackRate = 2.0;
                    arguments[0].muted = true;
                    arguments[0].play();
                """, video)

                # 等待播放完成
                self._wait_for_video_complete(video, duration)
            else:
                # 未知时长，播放10秒
                self.driver.execute_script("""
                    arguments[0].playbackRate = 2.0;
                    arguments[0].muted = true;
                    arguments[0].play();
                """, video)
                print(f"      ⏳ 等待 10 秒...")
                time.sleep(10)

        except Exception as e:
            print(f"      ⚠️ 视频播放失败: {str(e)[:50]}")

    def _monitor_popup_questions(self):
        """后台线程：监视视频弹窗题目并智能回答"""
        print("      [监视器] 开始监视弹窗...")
        check_interval = 0.5
        processed_popups = set()  # 避免重复处理同一弹窗

        while not self.stop_monitoring.is_set():
            try:
                popup = self._find_popup_question()

                if popup and popup.is_displayed():
                    # 生成弹窗唯一标识
                    popup_id = self._get_popup_id(popup)

                    if popup_id in processed_popups:
                        # 已处理过的弹窗，跳过
                        time.sleep(0.5)
                        continue

                    print("      [监视器] 🔔 检测到新弹窗题目！")

                    # 解析弹窗内容
                    question_data = self._parse_popup_question(popup)

                    if not question_data:
                        print("      [监视器] ⚠️ 未能解析题目")
                        continue

                    # 确定答案
                    if self.video_transcript and question_data['options']:
                        answer = self._intelligent_select_answer(question_data)
                    else:
                        # 无转录文本或无法解析，随机选择
                        answer = self._random_select(question_data)
                        print(f"      [监视器] 🎲 随机选择: {answer}")

                    # 点击答案
                    success = self._click_option(popup, answer)

                    if success:
                        print(f"      [监视器] ✓ 已选择: {answer}")
                        processed_popups.add(popup_id)

                        # 尝试提交
                        time.sleep(0.5)
                        self._click_submit_if_exists(popup)
                        time.sleep(1.0)  # 等待弹窗关闭
                    else:
                        print(f"      [监视器] ❌ 点击失败: {answer}")

            except Exception as e:
                # 静默处理，避免影响主线程
                pass

            self.stop_monitoring.wait(check_interval)

        print("      [监视器] 已停止")

    def _find_popup_question(self) -> Optional[Any]:
        """查找视频弹窗题目"""
        selectors = [
            '.video-box .popupBox .questionReplyBox',
            '.popupBox .question-common-abs-choice',
            '.questionReplyBox .question-common-abs-choice',
            '.video-popup .question-common-abs-choice',
            '.popupBox:has(.option)',  # 有选项的弹窗
        ]

        for selector in selectors:
            try:
                elements = self.driver.find_elements(By.CSS_SELECTOR, selector)
                for elem in elements:
                    if elem.is_displayed():
                        # 确认包含选项
                        options = elem.find_elements(By.CSS_SELECTOR, '.option, .option-wrap .option')
                        if len(options) >= 2:
                            return elem
            except:
                continue
        return None

    def _get_popup_id(self, popup) -> str:
        """生成弹窗唯一标识"""
        try:
            # 使用题目文本+选项文本的哈希
            text = popup.text
            return hashlib.md5(text[:200].encode()).hexdigest()[:16]
        except:
            return str(time.time())

    def _parse_popup_question(self, popup) -> Optional[Dict]:
        """解析弹窗中的题目和选项"""
        try:
            # 提取题目
            title_selectors = ['.ques-title', '.question-title', '.title', '.question-stem']
            title = ""
            for sel in title_selectors:
                try:
                    elem = popup.find_element(By.CSS_SELECTOR, sel)
                    title = elem.text.strip()
                    if title:
                        break
                except:
                    continue

            # 提取选项
            option_elems = popup.find_elements(By.CSS_SELECTOR,
                                               '.option.isNotReview, .option-wrap .option, .choice-option')

            options = []
            for i, opt_elem in enumerate(option_elems):
                try:
                    # 选项字母
                    letter_selectors = ['.caption', '.index', '.option-label', '.choice-label']
                    letter = ""
                    for sel in letter_selectors:
                        try:
                            letter_elem = opt_elem.find_element(By.CSS_SELECTOR, sel)
                            letter = letter_elem.text.strip().replace('.', '').replace(')', '').upper()
                            if letter:
                                break
                        except:
                            continue

                    if not letter:
                        letter = chr(65 + i)  # A, B, C, D...

                    # 选项内容
                    content_selectors = ['.content', '.option-content', '.text', '.choice-text']
                    content = ""
                    for sel in content_selectors:
                        try:
                            content_elem = opt_elem.find_element(By.CSS_SELECTOR, sel)
                            content = content_elem.text.strip()
                            if content:
                                break
                        except:
                            continue

                    if not content:
                        content = opt_elem.text.strip()

                    options.append({
                        'letter': letter,
                        'text': content,
                        'element': opt_elem
                    })

                except:
                    continue

            if not options:
                return None

            return {
                'question': title,
                'options': options
            }

        except Exception as e:
            print(f"      [监视器] 解析失败: {str(e)[:50]}")
            return None

    def _intelligent_select_answer(self, question_data: Dict) -> str:
        """基于视频内容智能选择答案"""
        question = question_data['question']
        options = question_data['options']

        print(f"      [监视器] 🤖 分析问题: {question[:50]}...")

        # 构建分析提示
        prompt = self._build_analysis_prompt(question, options)

        try:
            # 调用AI分析
            response = self.analyzer_client.chat.completions.create(
                model="kimi-k2-turbo-preview",  # 轻量级模型即可
                messages=[
                    {
                        "role": "system",
                        "content": "你是视频理解助手。根据视频内容选择最正确的答案，只返回选项字母，不要解释。"
                    },
                    {"role": "user", "content": prompt}
                ],
                temperature=0.1,
                max_tokens=5
            )

            answer_text = response.choices[0].message.content.strip().upper()

            # 从回答中提取字母
            valid_letters = [opt['letter'] for opt in options]
            for letter in valid_letters:
                if letter in answer_text:
                    print(f"      [监视器] ✨ AI选择: {letter}")
                    return letter

            # 如果没找到，使用关键词匹配备选
            return self._keyword_match(question, options)

        except Exception as e:
            print(f"      [监视器] ⚠️ AI分析失败: {str(e)[:50]}，使用关键词匹配")
            return self._keyword_match(question, options)

    def _build_analysis_prompt(self, question: str, options: List[Dict]) -> str:
        """构建AI分析提示词"""
        # 截断视频文本，避免过长
        transcript = self.video_transcript[:2000] if len(self.video_transcript) > 2000 else self.video_transcript

        prompt = f"""【视频内容】
        {transcript}

        【问题】
        {question}

        【选项】
        """
        for opt in options:
            prompt += f"{opt['letter']}. {opt['text']}\n"

        prompt += """
        【任务】
        根据视频内容，选择最正确的答案。只返回选项字母（如：A 或 B），不要任何解释。

        答案："""

        return prompt

    def _keyword_match(self, question: str, options: List[Dict]) -> str:
        """基于关键词匹配选择答案（备选策略）"""
        # 将问题和选项与视频文本匹配
        transcript_lower = self.video_transcript.lower()
        question_lower = question.lower()

        best_option = None
        best_score = -1

        for opt in options:
            opt_text = opt['text'].lower()

            # 计算匹配分数
            score = 0

            # 1. 选项文本在视频中的出现次数
            score += transcript_lower.count(opt_text) * 2

            # 2. 选项关键词（长度>3的词）匹配
            keywords = [w for w in opt_text.split() if len(w) > 3]
            for kw in keywords:
                if kw in transcript_lower:
                    score += 1

            # 3. 与问题的相关性（简单判断）
            if any(word in question_lower for word in opt_text.split()[:3]):
                score += 3

            if score > best_score:
                best_score = score
                best_option = opt

        if best_option:
            print(f"      [监视器] 🔍 关键词匹配: {best_option['letter']} (得分: {best_score})")
            return best_option['letter']

        # 默认选第一个
        return options[0]['letter'] if options else "A"

    def _random_select(self, question_data: Dict) -> str:
        """随机选择答案"""
        options = question_data.get('options', [])
        if not options:
            return "C"

        choice = random.choice(options)
        return choice['letter']

    def _click_option(self, popup, answer: str) -> bool:
        """点击指定选项"""
        try:
            # 查找对应字母的选项
            option_elems = popup.find_elements(By.CSS_SELECTOR,
                                               '.option.isNotReview, .option-wrap .option')

            for opt_elem in option_elems:
                try:
                    # 获取选项字母
                    letter_selectors = ['.caption', '.index', '.option-label']
                    letter = ""
                    for sel in letter_selectors:
                        try:
                            letter_elem = opt_elem.find_element(By.CSS_SELECTOR, sel)
                            letter = letter_elem.text.strip().replace('.', '').replace(')', '').upper()
                            if letter:
                                break
                        except:
                            continue

                    if letter == answer.upper():
                        # 滚动到可视区域并点击
                        self.driver.execute_script(
                            "arguments[0].scrollIntoView({block: 'center', behavior: 'smooth'});",
                            opt_elem
                        )
                        time.sleep(0.2)

                        # 尝试点击
                        try:
                            opt_elem.click()
                        except:
                            self.driver.execute_script("arguments[0].click();", opt_elem)

                        return True

                except:
                    continue

            # 如果没找到，尝试按索引点击（A=0, B=1...）
            try:
                idx = ord(answer.upper()) - ord('A')
                if 0 <= idx < len(option_elems):
                    opt_elem = option_elems[idx]
                    self.driver.execute_script(
                        "arguments[0].scrollIntoView({block: 'center', behavior: 'smooth'});",
                        opt_elem
                    )
                    time.sleep(0.2)
                    opt_elem.click()
                    return True
            except:
                pass

            return False

        except Exception as e:
            print(f"      [监视器] 点击失败: {str(e)[:50]}")
            return False

    def _click_submit_if_exists(self, popup):
        """点击提交按钮"""
        submit_selectors = [
            '.submit-btn', '.confirm-btn', '.ok-btn',
            'button[type="submit"]', '.popup-submit',
            '.questionReplyBox .submit'
        ]

        for selector in submit_selectors:
            try:
                btn = popup.find_element(By.CSS_SELECTOR, selector)
                if btn.is_displayed():
                    btn.click()
                    print("      [监视器] ✓ 已提交")
                    return True
            except:
                continue
        return False

    def _wait_for_video_complete(self, video, duration: float):
        """等待视频播放完成"""
        max_wait = duration / 2 + 30  # 考虑2倍速
        start_time = time.time()
        last_progress = 0

        while time.time() - start_time < max_wait:
            try:
                if self.stop_monitoring.is_set():
                    break

                current = self.driver.execute_script("return arguments[0].currentTime;", video)
                ended = self.driver.execute_script("return arguments[0].ended;", video)

                if ended or current >= duration - 1:
                    print(f"      ✓ 视频播放完成")
                    break

                # 每5秒报告进度
                elapsed = int(time.time() - start_time)
                if elapsed - last_progress >= 5:
                    print(f"      播放进度: {int(current)}/{int(duration)} 秒")
                    last_progress = elapsed

                time.sleep(0.5)

            except:
                break

    def _check_video_completed(self) -> bool:
        """检查视频是否已完成"""
        try:
            indicators = [
                '.video-completed', '.watched', '.finished',
                '[class*="completed"]', '[class*="finished"]'
            ]
            for indicator in indicators:
                if self.driver.find_elements(By.CSS_SELECTOR, indicator):
                    return True
            return False
        except:
            return False


class FlashcardHandler(ContentHandler):
    """单词闪卡处理器 """

    def __init__(self, driver):
        self.driver = driver

    def can_handle(self, question: Question) -> bool:
        return question.q_type == QuestionType.VOCABULARY_FLASHCARD

    def handle(self, question: Question) -> bool:
        print("    📚 处理单词闪卡...")
        max_cards = 100
        clicked = 0
        time.sleep(2)  # 等待页面完全加载
        for i in range(max_cards):
            try:
                next_btn = self._find_next_button()  # 每次循环重新查找按钮（DOM可能更新）
                if not next_btn:
                    print(f"      未找到下一个按钮，可能已完成（已点击{clicked}个）")
                    break
                if not next_btn.is_displayed() or not next_btn.is_enabled():  # 检查按钮是否真正可点击
                    print(f"      按钮不可用，完成")
                    break
                try:
                    disabled_next_button = self.driver.find_element(By.CSS_SELECTOR, '.action.next.disabled')
                    if disabled_next_button:
                        break
                except:
                    pass
                # 滚动到可视区域
                self.driver.execute_script(
                    "arguments[0].scrollIntoView({block: 'center', behavior: 'smooth'});",
                    next_btn
                )
                time.sleep(0.5)
                # 点击
                try:
                    next_btn.click()
                except:
                    self.driver.execute_script("arguments[0].click();", next_btn)
                clicked += 1
                # 等待动画完成
                time.sleep(0.5)  # 闪卡切换动画时间
                current_word = self.driver.find_element(By.XPATH,
                                                        '//*[@id="question-vocabulary-base-id"]/div/div[2]/div')
                print(f" 学习{current_word.text}")
            except Exception as e:
                error_msg = str(e)
                print(f"      处理闪卡失败: {error_msg[:50]}")
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
                # 尝试继续
                time.sleep(1)
                continue

        print(f"    ✅ 单词闪卡完成，共 {clicked} 个")
        return True  # 返回True表示已处理

    def _find_next_button(self):
        """查找下一个按钮 - 每次重新查询"""
        selectors = [
            '.vocActions .next',
            '.action.next',
            '.next-btn',
            '.vocabulary-actions .next',
            'button.next',
            '.flashcard-next',
            '[class*="next"]:not([class*="disabled"])',
        ]

        for selector in selectors:
            try:
                elems = self.driver.find_elements(By.CSS_SELECTOR, selector)
                for elem in elems:
                    if elem.is_displayed():
                        return elem
            except:
                continue
        return None


# ==================== 主求解器 ====================
class AISolver:
    """AI答题器 - 协调解析、构建、执行流程"""

    def __init__(self, driver, config: Config):
        self.driver = driver
        self.config = config
        self.kimi = KimiClient(self.config)
        self.parser = QuestionParser(driver, self.config.whisper_api)
        self.prompt_builder = PromptBuilder(self.kimi)
        self.executor = AnswerExecutor(driver)
        self.content_handlers: List[ContentHandler] = [
            FlashcardHandler(driver),
            DiscussionBoardHandler(driver),
        ]
        # 只存储已处理的内容哈希
        self.processed_hashes: set = set()
        self._processed_video_tabs: set = set()
        self._processed_audio_tabs: set = set()


    def solve_current_chapter(self, chapter_name: str) -> bool:
        """处理当前章节的所有Tab - 累积原文模式"""
        print(f"\n{'=' * 60}")
        print(f"📚 开始处理章节: {chapter_name}")
        print(f"{'=' * 60}")

        # 只初始化章节，不发送原文
        self.kimi.start_new_chapter(chapter_name)

        # 遍历所有Tab
        level1_tabs = self._get_level1_tabs()

        for l1_idx, l1_tab in enumerate(level1_tabs):
            print(f"📂 一级Tab [{l1_idx}]: {l1_tab['title']}")
            # 切换一级TAB时清空ai对话历史
            if l1_idx > 0:
                self.kimi.force_reset(f"{chapter_name}_{l1_tab['title']}")
                print(f"   🔄 切换一级Tab，已清空AI对话历史")
            if not WebDriverHelper.safe_click(self.driver, l1_tab['element']):
                continue
            time.sleep(1.5)

            level2_tabs = self._get_level2_tabs()

            if not level2_tabs:
                # 无二级Tab，处理并累积原文
                self._process_tab_with_accumulation(f"{l1_tab['title']}", l1_idx, 0)
            else:
                for l2_idx, l2_tab in enumerate(level2_tabs):
                    print(f"\n  📄 二级Tab [{l2_idx}]: {l2_tab['title']}")

                    if not WebDriverHelper.safe_click(self.driver, l2_tab['element']):
                        continue
                    time.sleep(1.5)

                    tab_name = f"{l1_tab['title']}_{l2_tab['title']}"
                    self._process_tab_with_accumulation(tab_name, l1_idx, l2_idx)

                    # 重新获取DOM引用
                    level2_tabs = self._get_level2_tabs()
                    if l2_idx < len(level2_tabs):
                        l2_tab['element'] = level2_tabs[l2_idx]['element']

        print(f"\n{'=' * 60}")
        print(f"✅ 章节 {chapter_name} 处理完成")
        print(f"{'=' * 60}")
        return True

    def _process_tab_with_accumulation(self, tab_name: str, l1_idx: int, l2_idx: int) -> bool:
        """处理Tab - 累积原文模式，包含视频预处理"""

        # ========== 视频，音频预处理：播放并转录，为后续题目提供上下文 ==========
        self._preprocess_video_if_needed(tab_name, l1_idx, l2_idx)
        self._preprocess_audio_if_needed(tab_name, l1_idx, l2_idx)

        # 原逻辑：提取文本材料（阅读文章等）
        current_passage = self._extract_passage()
        if current_passage:
            self.kimi.add_passage_if_new(current_passage)

        return self._process_current_tab_content(
            self.kimi.current_chapter_id or "unknown", tab_name, l1_idx, l2_idx
        )

    def _preprocess_audio_if_needed(self, tab_name: str, l1_idx: int, l2_idx: int):
        """检测并预处理音频：播放（静音）、转录，并将转录文本注入 AI 上下文"""
        # 避免重复处理同一 Tab 的音频
        if (tab_name, l1_idx, l2_idx) in self._processed_audio_tabs:
            return

        if not self._has_audio_on_page():
            return

        print("   🎵 检测到音频，开始预处理（下载+转录）...")
        try:
            audio_url = self._extract_audio_url_from_page()
            if not audio_url:
                print("   ⚠️ 未找到有效音频URL，跳过")
                return

            transcriber = AudioTranscriber(use_local=True)  # 或传入 config.whisper_api

            # 获取音频时长，用于选择识别方法
            duration = self._get_audio_duration()
            if duration > 120:  # 长音频用分段识别方法（已优化本地模型不分段）
                transcript = transcriber.transcribe_long_audio(audio_url, language="en")
            else:
                transcript = transcriber.transcribe(audio_url, language="en")

            if transcript:
                self.kimi.add_passage_if_new(transcript)
                print(f"   📄 已将音频转录（{len(transcript)}字符）加入上下文")
            else:
                print("   ⚠️ 音频转录为空")

            self._processed_audio_tabs.add((tab_name, l1_idx, l2_idx))

        except Exception as e:
            print(f"   ⚠️ 音频预处理失败: {str(e)[:80]}")
            logger.error(f"音频预处理异常: {e}", exc_info=True)

    def _has_audio_on_page(self) -> bool:
        """检测页面是否包含音频元素或音频材料"""
        try:
            # 方法1：传统 <audio> 标签
            audios = self.driver.find_elements(By.TAG_NAME, 'audio')
            if any(a.is_displayed() or a.get_attribute('src') for a in audios):
                return True

            # 方法2：音频材料容器（U校园常见）
            audio_containers = self.driver.find_elements(By.CSS_SELECTOR, '.audio-material-wrapper, .question-audio')
            if audio_containers:
                return True

            # 方法3：direction 关键词提示（备选）
            try:
                direction = self.driver.find_element(By.CSS_SELECTOR, '.layout-direction-container, .abs-direction')
                text = direction.text.lower()
                if any(kw in text for kw in ['listen', 'audio', 'hear', 'talk', 'conversation']):
                    # 再确认确实有音频资源
                    if self._extract_audio_url_from_page():
                        return True
            except:
                pass

            return False
        except:
            return False

    def _extract_audio_url_from_page(self) -> Optional[str]:
        """从页面提取音频URL（复用 ListeningFillInStrategy 的逻辑）"""
        try:
            # 先找 <audio> 标签
            audio_elem = self.driver.find_element(By.CSS_SELECTOR, 'audio')
            src = audio_elem.get_attribute('src')
            if src:
                return src.split('#')[0]

            # 再查 source 子标签
            sources = self.driver.find_elements(By.CSS_SELECTOR, 'audio source')
            for source in sources:
                src = source.get_attribute('src')
                if src:
                    return src.split('#')[0]

            # 尝试从网络请求中找（需要 selenium-wire 等，暂略）
            return None
        except:
            return None

    def _get_audio_duration(self) -> float:
        """获取音频时长（秒），用于判断是否长音频"""
        try:
            audio = self.driver.find_element(By.TAG_NAME, 'audio')
            duration = self.driver.execute_script("return arguments[0].duration;", audio)
            return float(duration) if duration else 0
        except:
            return 0  # 未知时长

    def _preprocess_video_if_needed(self, tab_name: str, l1_idx: int, l2_idx: int):
        """检测并预处理视频：播放、转录、处理弹窗，并将转录文本注入 AI 上下文"""
        # 避免重复处理同一 Tab 的视频
        if (tab_name, l1_idx, l2_idx) in self._processed_video_tabs:
            return

        if not self._has_video_on_page():
            return

        print("   🎬 检测到视频，开始预处理（播放+转录）...")
        try:
            # 创建一个独立的 VideoHandler，不依赖 question 对象
            video_handler = VideoHandler(self.driver, self.config)

            # 播放视频并处理弹窗（内部会启动监视器）
            video_handler._play_video_and_handle_popups()

            # 获取转录文本并注入上下文
            transcript = video_handler.video_transcript
            if transcript:
                self.kimi.add_passage_if_new(transcript)
                print(f"   📄 已将视频转录（{len(transcript)}字符）加入上下文")
            else:
                print("   ⚠️ 未获得视频转录，后续题目可能缺乏上下文")

            # 记录已处理，防止重复
            self._processed_video_tabs.add((tab_name, l1_idx, l2_idx))

        except Exception as e:
            print(f"   ⚠️ 视频预处理失败: {str(e)[:80]}")
            logger.error(f"视频预处理异常: {e}", exc_info=True)

    def _has_video_on_page(self) -> bool:
        """检测当前页面是否包含可见的视频元素"""
        try:
            videos = self.driver.find_elements(By.TAG_NAME, 'video')
            return any(v.is_displayed() for v in videos)
        except:
            return False

    def _process_current_tab_content(self, chapter_name: str, tab_name: str, l1_idx: int, l2_idx: int) -> bool:
        """处理当前Tab页面（支持动态加载的多页题目）"""

        # 生成唯一标识：章节路径 + direction内容
        # 这样即使 direction 相同，不同章节也不会冲突
        direction_part = self._generate_content_hash_from_direction()
        if direction_part == "empty":
            direction_part = "no_direction"

        # 关键：章节路径 + direction，确保唯一性
        content_hash = f"{chapter_name}|{tab_name}|{l1_idx}|{l2_idx}|{direction_part}"

        print(f"   🔑 内容标识: {hashlib.md5(content_hash.encode()).hexdigest()[:16]}...")

        if content_hash in self.processed_hashes:
            print(f"   ⏭️ 已处理过，跳过")
            return False

        self.processed_hashes.add(content_hash)

        page_num = 1
        total_answered = 0
        last_questions_signature = ""  # 用于检测内容是否真正变化

        while True:
            questions, directions = self.parser.parse_all()
            print(f"\n   📄 处理第 {page_num} 页题目...")

            # 解析当前可见题目
            print(f"   📊 找到 {len(questions)} 个可见题目")

            # 生成签名检测变化
            current_signature = self._generate_questions_signature(questions)

            if current_signature == last_questions_signature and page_num > 1:
                print(f"   ⚠️ 题目内容与上次相同，可能已到达最后一页")
                break

            last_questions_signature = current_signature

            # 处理特殊内容
            special_handled = False
            for q in questions:
                for handler in self.content_handlers:
                    if handler.can_handle(q):
                        print(f"    🎯 使用 {handler.__class__.__name__} 处理")
                        handler.handle(q)
                        special_handled = True
                        if q.q_type in [QuestionType.VOCABULARY_FLASHCARD]:
                            print(f"   ✅ 特殊内容处理完成")
                            return True
                        break

            # 过滤普通题目
            normal_questions = [q for q in questions if q.q_type not in [
                QuestionType.VOCABULARY_FLASHCARD,
                QuestionType.DISCUSSION_BOARD
            ]]

            # AI答题
            if normal_questions:
                print(f"   📝 共 {len(normal_questions)} 道题目需要回答")
                prompt = self.prompt_builder.build(normal_questions, directions)
                ai_response = self.kimi.ask(prompt)

                if ai_response:
                    success_count = 0

                    for q in normal_questions:
                        # 根据题型选择解析方式
                        if q.q_type in [QuestionType.SINGLE_CHOICE, QuestionType.MULTIPLE_CHOICE,
                                        QuestionType.VOCABULARY_TEST]:
                            # 选择题：从ai_response中提取该题的答案
                            ans = self._extract_single_answer(ai_response, q.number)
                        elif q.q_type in [QuestionType.BANKED_CLOZE, QuestionType.FILL_IN,
                                          QuestionType.DROPDOWN_SELECT]:
                            # 填空题：直接传完整AI回答，让executor自己解析
                            ans = ai_response
                        else:
                            # 其他类型：直接传
                            ans = ai_response

                        if ans:
                            result = self.executor.execute(q, ans)
                            if result.success:
                                success_count += 1
                        else:
                            print(f"   ⚠️ 题目 {q.number} 无答案")

                    total_answered += success_count
                    print(f"   ✅ 本页成功填写 {success_count}/{len(normal_questions)} 题")

            # 快速提交
            if normal_questions and self.executor.submit():
                time.sleep(0.3)  # 减少等待
                self._handle_confirm_dialog()

            # 查找下一题按钮
            next_btn = self._find_next_question_button()
            if not next_btn:
                print(f"   ✅ 没有更多题目了")
                break

            print(f"   ➡️ 点击下一题...")
            pre_click_signature = current_signature

            if not WebDriverHelper.safe_click(self.driver, next_btn):
                print(f"   ❌ 点击下一题失败")
                break

            # 快速检测内容变化
            if not self._wait_for_content_change(pre_click_signature, timeout=5):
                print(f"   ⚠️ 内容未变化，可能已到最后一页")
                break

            page_num += 1

            if page_num > 50:
                print(f"   ⚠️ 达到最大页数限制，停止")
                break
        print(f"   ✅ 总共回答 {total_answered} 题")
        return True

    def _extract_single_answer(self, ai_response: str, question_number: int) -> str:
        """从AI回答中提取指定题目的答案"""
        # 匹配 "1.A" 或 "1. A" 或 "1)A" 等格式
        pattern = rf'{question_number}\s*[.、\)\]]\s*([A-D]+)'
        match = re.search(pattern, ai_response, re.IGNORECASE)
        if match:
            return match.group(1).upper()

        # 如果没找到序号，尝试按行分割，取第question_number个
        lines = [l.strip() for l in ai_response.split('\n') if l.strip()]
        if question_number <= len(lines):
            # 提取行中的字母
            line = lines[question_number - 1]
            letters = re.findall(r'[A-D]', line.upper())
            return ''.join(letters) if letters else line

        return ""

    def _generate_questions_signature(self, questions: List[Question]) -> str:
        """生成题目签名，用于检测内容变化"""
        if not questions:
            return "empty"

        # 使用题目编号+类型+前30字符生成签名
        parts = []
        for q in questions:
            q_type = q.q_type.name if q.q_type else "UNKNOWN"
            text_preview = q.text[:30] if q.text else ""
            parts.append(f"{q.number}:{q_type}:{text_preview}")

        return hashlib.md5("|".join(parts).encode()).hexdigest()[:16]

    def solve_current_page(self, chapter_name: str = "unknown") -> bool:
        """处理当前页面（用于侧边栏切换后的处理）"""
        print("\n" + "=" * 60)
        print("📚 开始处理当前页面")
        print("=" * 60)

        # 生成基于内容的状态标识
        state_key = self._generate_content_hash()

        # 如果无法获取内容哈希，使用章节名+时间戳
        if not state_key or state_key == "empty":
            state_key = f"{chapter_name}_{int(time.time())}"

        print(f"   🔑 内容标识: {state_key[:50]}...")

        if state_key in self.processed_hashes:
            print(f"   ⏭️ 已处理过相同内容，跳过")
            return False

        self.processed_hashes.add(state_key)

        # 处理当前页面内容
        self._process_current_content(chapter_name)

        print(f"\n{'=' * 60}")
        print("✅ 当前页面处理完成")
        print(f"{'=' * 60}")
        return True

    def solve(self) -> bool:
        """完整答题流程（扫描所有Tab）"""
        print("\n" + "=" * 60)
        print("📚 开始完整答题流程")
        print("=" * 60)

        # 获取所有一级Tab
        level1_tabs = self._get_level1_tabs()

        if not level1_tabs:
            # 无Tab结构，直接处理当前页
            return self.solve_current_page("default")

        for l1_idx, l1_tab in enumerate(level1_tabs):
            print(f"\n{'=' * 60}")
            print(f"📂 一级Tab [{l1_idx}]: {l1_tab['title']}")
            print(f"{'=' * 60}")

            # 点击一级Tab
            if not WebDriverHelper.safe_click(self.driver, l1_tab['element']):
                continue
            time.sleep(1.5)

            # 获取并遍历二级Tab
            level2_tabs = self._get_level2_tabs()

            if not level2_tabs:
                # 无二级Tab，直接处理
                self._process_tab_content(l1_tab['title'], "default", (l1_idx, 0))
            else:
                for l2_idx, l2_tab in enumerate(level2_tabs):
                    print(f"\n  📄 二级Tab [{l2_idx}]: {l2_tab['title']}")

                    # 点击二级Tab
                    if not WebDriverHelper.safe_click(self.driver, l2_tab['element']):
                        continue
                    time.sleep(1.5)

                    # 处理当前页
                    self._process_tab_content(l1_tab['title'], l2_tab['title'], (l1_idx, l2_idx))

                    # 重新获取二级Tab列表（防止DOM变化）
                    level2_tabs = self._get_level2_tabs()
                    # 更新当前元素引用
                    if l2_idx < len(level2_tabs):
                        l2_tab['element'] = level2_tabs[l2_idx]['element']

        print(f"\n{'=' * 60}")
        print("✅ 答题流程完成")
        print(f"{'=' * 60}")
        return True

    def _find_next_question_button(self) -> Optional[Any]:
        """查找"下一题"按钮 - 增强版本"""
        selectors = [
            '.next-question-btn:not(.disabled)',
            '.btn-next:not([disabled])',
            'button.next:not(.disabled)',
            '.pagination-next:not(.disabled)',
            '.question-next:not(.disabled)',
            '.action.next:not(.disabled)',
            '.submit-bar-pc--btn-next:not(.disabled)',
            '.next-btn:not(.disabled)',
            '[class*="next"]:not(.disabled)',
        ]

        for selector in selectors:
            try:
                btns = self.driver.find_elements(By.CSS_SELECTOR, selector)
                for btn in btns:
                    if btn.is_displayed() and btn.is_enabled():
                        # 检查是否真的是"下一题"而不是"下一个章节"
                        text = btn.text.lower()
                        aria_label = (btn.get_attribute('aria-label') or '').lower()
                        if any(k in text or k in aria_label for k in ['下一题', 'next', '下一页', 'next question']):
                            return btn
            except Exception as e:
                error_msg = str(e)
                print(f"操作失败: {error_msg[:50]}")  # 控制台只显示简短信息
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
                continue

        # 备选：查找包含特定文本的按钮
        try:
            all_btns = self.driver.find_elements(By.TAG_NAME, 'button')
            for btn in all_btns:
                if not btn.is_displayed():
                    continue
                text = btn.text.lower()
                if any(k in text for k in ['下一题', 'next question', '下一页', 'next']):
                    if 'submit' not in text and '提交' not in text:
                        return btn
        except:
            pass

        return None

    def _generate_content_hash_from_direction(self) -> str:
        """基于direction生成内容标识（题目会变化，但direction不变）"""
        try:
            direction_elem = WebDriverHelper.safe_find_element(
                self.driver,
                ['.abs-direction', '.layout-direction-container', '.direction-container']
            )
            if direction_elem:
                text = direction_elem.text.strip()
                if text:

                    return hashlib.md5(text.encode()).hexdigest()[:16]
        except:
            pass
        return "empty"

    def _generate_content_hash(self) -> str:
        """基于页面内容生成唯一标识"""
        try:
            # 方法1：使用direction文本
            direction_elem = WebDriverHelper.safe_find_element(
                self.driver,
                ['.abs-direction', '.layout-direction-container', '.discussion-title']
            )
            if direction_elem:
                text = direction_elem.text.strip()
                if text:
                    return hashlib.md5(text.encode()).hexdigest()[:16]

            # 方法2：使用题目文本
            questions, _ = self.parser.parse_all()
            if questions:
                content = "|".join([f"{q.number}:{q.text[:30]}" for q in questions[:3]])
                return hashlib.md5(content.encode()).hexdigest()[:16]

            # 方法3：使用页面主要内容
            body = self.driver.find_element(By.TAG_NAME, 'body').text[:300]
            return hashlib.md5(body.encode()).hexdigest()[:16]

        except Exception as e:
            error_msg = str(e)
            print(f"   ⚠️ 生成哈希失败:{error_msg[:50]} ")
            logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
            return "empty"

    def _process_tab_content(self, l1_title: str, l2_title: str, tab_indices: Tuple[int, int]):
        """处理Tab页面内容"""
        # 生成内容哈希
        state_key = self._generate_content_hash()

        if not state_key or state_key == "empty":
            state_key = f"{l1_title}_{l2_title}_{tab_indices[0]}_{tab_indices[1]}"

        print(f"   🔑 内容标识: {state_key[:50]}...")

        if state_key in self.processed_hashes:
            print(f"   ⏭️ 已处理过，跳过")
            return

        self.processed_hashes.add(state_key)

        chapter_name = f"{l1_title}_{l2_title}" if l2_title != "default" else l1_title
        self._process_current_content(chapter_name)

    def _process_current_content(self, chapter_name: str):
        """处理当前页面内容（核心逻辑）- 适配累积原文模式"""
        print(f"   📝 处理: {chapter_name}")

        # 解析题目
        questions, directions = self.parser.parse_all()
        print(f"   📊 找到 {len(questions)} 个题目")

        # 处理特殊内容（视频、闪卡）
        normal_questions = []
        for q in questions:
            handled = False
            for handler in self.content_handlers:
                if handler.can_handle(q):
                    handler.handle(q)
                    handled = True
                    break
            if not handled:
                normal_questions.append(q)

        if not normal_questions:
            print("    ℹ️ 无需要AI回答的题目")
            return

        # AI答题
        print(f"    📝 共 {len(normal_questions)} 道题目需要回答")

        # 构建Prompt并提问
        prompt = self.prompt_builder.build(normal_questions, directions)
        ai_response = self.kimi.ask(prompt)

        if not ai_response:
            print("    ❌ AI未返回答案")
            return

        # 解析并填写答案
        answers = self._parse_ai_response(ai_response, len(normal_questions))
        success_count = 0

        for q, ans in zip(normal_questions, answers):
            if ans:
                result = self.executor.execute(q, ans)
                if result.success:
                    success_count += 1

        print(f"    ✅ 成功填写 {success_count}/{len(normal_questions)} 题")

        # 提交
        if self.executor.submit():
            time.sleep(1.5)
            self._handle_confirm_dialog()

    def _get_level1_tabs(self) -> List[Dict]:
        """获取一级Tab"""
        tabs = []
        elements = WebDriverHelper.safe_find_elements(self.driver, Selectors.LEVEL1_TABS)
        seen = set()
        for elem in elements:
            try:
                title = elem.get_attribute('title') or elem.text.strip().split('\n')[0]
                if title and title not in seen and len(title) < 50:
                    seen.add(title)
                    tabs.append({
                        'element': elem,
                        'title': title,
                        'is_active': 'activity' in (elem.get_attribute('class') or '').lower()
                    })
            except:
                continue
        return tabs

    def _get_level2_tabs(self) -> List[Dict]:
        """获取二级Tab"""
        tabs = []
        container = WebDriverHelper.safe_find_element(
            self.driver,
            ['.pc-header-tasks-container', '.pc-header-tasks-layout']
        )
        if not container:
            return tabs
        elements = WebDriverHelper.safe_find_elements(
            self.driver,
            Selectors.LEVEL2_TABS,
            parent=container
        )
        seen = set()
        for elem in elements:
            try:
                title = elem.text.strip().split('\n')[0]
                if title and title not in seen and len(title) < 50:
                    seen.add(title)
                    tabs.append({
                        'element': elem,
                        'title': title,
                        'is_active': 'activity' in (elem.get_attribute('class') or '').lower()
                    })
            except:
                continue
        return tabs

    def _extract_passage(self) -> str:
        """提取阅读材料"""
        selectors = [
            '.question-common-abs-material',
            '.text-material-wrapper',
            '.reading-passage',
            '.passage-content',
        ]

        for selector in selectors:
            try:
                elems = self.driver.find_elements(By.CSS_SELECTOR, selector)
                for elem in elems:
                    text = elem.text.strip()
                    if len(text) > 200:
                        return text
            except:
                continue

        return ""

    def _parse_ai_response(self, response: str, expected_count: int, q_type: QuestionType = None) -> List[str]:
        """解析AI回答"""
        answers = []
        response = response.strip()

        # 策略1: 带序号的格式 "1.A 2.B" 或 "1.AB 2.C"
        pattern1 = r'(\d+)\s*[.、\)\]]\s*([A-Za-z]+)(?=\s*\d+\s*[.、\)\]]|$)'
        matches = re.findall(pattern1, response, re.DOTALL | re.IGNORECASE)

        if matches and len(matches) >= expected_count:
            match_dict = {}
            for num, content in matches:
                idx = int(num) - 1
                clean = content.upper().strip()
                match_dict[idx] = clean

            for i in range(expected_count):
                answers.append(match_dict.get(i, ''))
            return answers

        # 策略2: 无序号，纯字母列表（空格/逗号分隔）如 "C A A" 或 "C, A, A"
        # 提取所有单词（True/False/Not given 或 A/B/C/D）
        words = re.findall(r'\b(True|False|Not\s*given|Not\s*mentioned|[A-Z])\b',
                           response, re.IGNORECASE)

        if len(words) >= expected_count:
            return [w.upper() for w in words[:expected_count]]

        # 策略3: 回退 - 按行分割
        lines = [line.strip() for line in response.split('\n') if line.strip()]
        for line in lines[:expected_count]:
            # 提取行中的第一个字母或单词
            match = re.search(r'\b(True|False|Not\s*given|[A-Z])\b', line, re.IGNORECASE)
            answers.append(match.group(1).upper() if match else '')

        # 补齐
        while len(answers) < expected_count:
            answers.append('')

        return answers[:expected_count]

    def _handle_confirm_dialog(self):
        """处理确认弹窗"""
        try:
            buttons = self.driver.find_elements(By.TAG_NAME, 'button')
            for btn in buttons:
                text = btn.text.strip()
                if any(k in text for k in ['确认', '确定', '我知道了', '继续', 'OK']):
                    if btn.is_displayed():
                        btn.click()
                        time.sleep(1)
                        return True
        except:
            pass
        return False

    def _wait_for_content_change(self, previous_signature: str, timeout: int = 10) -> bool:
        """等待页面内容变化（用于检测分页加载）"""
        start_time = time.time()
        check_interval = 0.5

        while time.time() - start_time < timeout:
            try:
                # 重新解析当前题目生成签名
                questions, _ = self.parser.parse_all()
                current_signature = self._generate_questions_signature(questions)

                # 如果签名不同，说明内容已变化
                if current_signature != previous_signature and current_signature != "empty":
                    print(f"        ✓ 内容已变化: {previous_signature[:8]}... -> {current_signature[:8]}...")
                    return True

            except Exception as e:
                logger.debug(f"等待内容变化时出错: {e}")

            time.sleep(check_interval)

        print(f"        ⚠️ 等待内容变化超时")
        return False


# ==================== 课程学习器 ====================
class CourseLearner:
    """课程学习器 - 首次进入扫描，之后通过右侧列表切换课程"""

    def __init__(self, driver, config: Config):
        self.driver = driver
        self.config = config
        self.solver = AISolver(driver, config)
        self.chapters: List[Dict] = []
        self.current_chapter_index: int = -1
    def learn(self) -> bool:
        """主学习流程"""
        # 第一步：首次进入
        if not self._first_entry():
            return False

        # 第二步：处理当前章节的所有Tab
        current_chapter = self.chapters[self.current_chapter_index]
        print(f"\n📖 处理章节: {current_chapter['name']}")

        # 处理当前章节的所有Tab
        self.solver.solve_current_chapter(current_chapter['name'])

        # 第三步：侧边栏切换到下一章节
        return self._sidebar_learning_loop()

    def _first_entry(self) -> bool:
        """首次进入：扫描目录并进入第一个未完成章节"""
        print("🚀 首次进入模式：扫描课程目录...")
        try:

            input("请手动点击进入要刷的课后按回车（只进入目录不进入学习页面）")
            winsound.MessageBeep()

            # 扫描所有章节
            if not self._scan_all_chapters():
                return False

            # 找到第一个未完成章节并进入
            target_chapter = None
            for chapter in self.chapters:
                if self.config.learning_strategy == "learn_all_compulsory_course" and not chapter['is_compulsory']:
                    print(f"  ⏭️ 跳过非必修: {chapter['name']}")
                    continue
                if chapter['state'] in ["未开始", "进行中"]:
                    target_chapter = chapter
                    break

            if not target_chapter:
                print("✅ 所有章节已完成")
                return False

            print(f"🎯 首次进入章节: {target_chapter['name']} (Unit {target_chapter['unit_index'] + 1})")

            # 点击进入
            if not self._enter_chapter_by_index(target_chapter['index']):
                return False

            self.current_chapter_index = target_chapter['index']
            time.sleep(3)

            return True

        except Exception as e:
            error_msg = str(e)
            print(f"❌ 首次进入失败: {error_msg[:100] if len(error_msg) > 100 else error_msg}")
            logger.error(f"详细错误: {error_msg}", exc_info=True)
            return False

    def _scan_all_chapters(self) -> bool:
        """扫描所有章节（仅首次使用）"""
        try:
            unit_container = WebDriverWait(self.driver, 10).until(
                EC.presence_of_element_located((By.CLASS_NAME, 'unipus-tabs_unitTabScrollContainer__fXBxR'))
            )
            unit_tabs = unit_container.find_elements(By.CSS_SELECTOR, ':scope > *')

            print(f"🔍 扫描 {len(unit_tabs)} 个单元...")

            chapter_idx = 0
            for unit_idx, unit_tab in enumerate(unit_tabs):
                unit_tab.click()
                time.sleep(1)

                current_frame = self.driver.find_element(By.CLASS_NAME, 'unipus-tabs_itemActive__x0WVI')
                chapters = current_frame.find_elements(By.CLASS_NAME, 'courses-unit_taskItemInnerLayout__DTYuN')

                for chapter in chapters:
                    try:
                        name_elem = chapter.find_element(By.CLASS_NAME, 'courses-unit_taskTypeName__99BXj')

                        # 检查是否必修
                        try:
                            chapter.find_element(By.CLASS_NAME, 'courses-unit_taskRequireIcon__zZldK')
                            is_compulsory = True
                        except NoSuchElementException:
                            is_compulsory = False

                        state_elem = chapter.find_element(By.CLASS_NAME, 'courses-unit_nodePassStateTip__O3coH')

                        self.chapters.append({
                            'index': chapter_idx,
                            'unit_index': unit_idx,
                            'name': name_elem.text,
                            'is_compulsory': is_compulsory,
                            'state': state_elem.text,
                            'element': name_elem
                        })
                        chapter_idx += 1

                    except Exception as e:
                        error_msg = str(e)
                        print(f"操作失败: {error_msg[:50]}")  # 控制台只显示简短信息
                        logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
                        continue

            print(f"✅ 扫描完成，共 {len(self.chapters)} 个章节")
            return True

        except Exception as e:
            error_msg = str(e)
            print(f"❌ 扫描失败: {error_msg[:50]}")
            logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
            return False

    def _sidebar_learning_loop(self) -> bool:
        """侧边栏导航学习循环"""
        print("\n" + "=" * 60)
        print("🔄 进入侧边栏导航模式")
        print("=" * 60)

        while True:
            # 查找下一章节
            next_chapter = self._find_next_by_sidebar()

            if not next_chapter:
                print("✅ 没有更多章节")
                break

            print(f"\n➡️ 切换到: {next_chapter['name']}")

            if self._click_sidebar_chapter(next_chapter):
                # 删除AI对话历史
                self.solver.kimi.force_reset(next_chapter['name'])
                # 更新索引
                for i, ch in enumerate(self.chapters):
                    if ch['name'] == next_chapter['name']:
                        self.current_chapter_index = i
                        break

                # 等待页面加载
                time.sleep(3)

                # 处理新章节的所有Tab
                current_chapter = self.chapters[self.current_chapter_index]
                print(f"\n📖 处理章节: {current_chapter['name']}")
                self.solver.solve_current_chapter(current_chapter['name'])

                time.sleep(2)
            else:
                print("❌ 切换失败")
                break

        print("\n🎉 学习完成！")
        return True

    def _find_next_by_sidebar(self) -> Optional[Dict]:
        """通过侧边栏查找下一章节"""
        sidebar = WebDriverHelper.safe_find_element(self.driver, Selectors.SIDEBAR)
        if not sidebar:
            return None

        nodes = WebDriverHelper.safe_find_elements(self.driver, Selectors.SIDEBAR_NODES, parent=sidebar)

        # 找到当前激活的节点
        current_idx = -1
        for i, node in enumerate(nodes):
            try:
                is_active = (
                        'active' in (node.get_attribute('class') or '').lower() or
                        'pc-menu-activity' in (node.get_attribute('class') or '')
                )
                if is_active:
                    current_idx = i
                    break
            except:
                continue

        # 查找下一个未完成的
        for i in range(current_idx + 1, len(nodes)):
            try:
                node = nodes[i]

                # 获取名称
                name_elem = node.find_element(By.CSS_SELECTOR, '.pc-menu-node-name, span, .name')
                name = name_elem.text.strip().split('\n')[0]

                # 检查是否已完成
                if self._check_node_completed(node):
                    print(f"  ⏭️ 跳过已完成: {name}")
                    continue

                # 检查是否锁定
                if self._check_node_locked(node):
                    print(f"  🔒 跳过锁定: {name}")
                    continue

                return {
                    'index': i,
                    'name': name,
                    'element': node,
                    'element_index': i
                }

            except Exception as e:
                error_msg = str(e)
                print(f"操作失败: {error_msg[:50]}")  # 控制台只显示简短信息
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
                continue
        return None

    def _check_node_completed(self, node) -> bool:
        """检查节点是否已完成"""
        try:
            indicators = ['.complete', '.finished', '.done', '.passed']
            for ind in indicators:
                if node.find_elements(By.CSS_SELECTOR, ind):
                    return True
            text = node.text.lower()
            return any(kw in text for kw in ['已完成', 'completed', 'done', '100%'])
        except:
            return False

    def _check_node_locked(self, node) -> bool:
        """检查节点是否锁定"""
        try:
            locked_indicators = ['.lock', '.locked', '.disabled']
            for ind in locked_indicators:
                if node.find_elements(By.CSS_SELECTOR, ind):
                    return True
            icons = node.find_elements(By.CSS_SELECTOR, 'svg, i.icon')
            for icon in icons:
                if 'lock' in (icon.get_attribute('class') or '').lower():
                    return True
        except:
            pass
        return False

    def _click_sidebar_chapter(self, chapter: Dict) -> bool:
        """点击侧边栏章节"""
        return WebDriverHelper.safe_click(self.driver, chapter['element'])

    def _enter_chapter_by_index(self, index: int) -> bool:
        """通过索引进入章节（首次使用）"""
        if index >= len(self.chapters):
            return False
        chapter = self.chapters[index]
        # 先点击Unit
        try:
            unit_container = self.driver.find_element(By.CLASS_NAME, 'unipus-tabs_unitTabScrollContainer__fXBxR')
            unit_tabs = unit_container.find_elements(By.CSS_SELECTOR, ':scope > *')
            if chapter['unit_index'] < len(unit_tabs):
                unit_tabs[chapter['unit_index']].click()
                time.sleep(1)
        except Exception as e:
            error_msg = str(e)
            print(f"  ⚠️ 点击Unit失败: {error_msg[:50]}")
            logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
        # 点击章节
        try:
            self.driver.execute_script("arguments[0].click();", chapter['element'])
            print(f"✅ 章节点击成功")
            return True
        except Exception as e:
            error_msg = str(e)
            print(f"❌ 章节点击失败: {error_msg[:50]}")
            return False

    def _try_alternative_navigation(self) -> bool:
        """备用导航方案"""
        # 尝试页面内下一题按钮
        try:
            next_btn = self.driver.find_element(By.CSS_SELECTOR, '.action.next, .next-btn')
            next_btn.click()
            time.sleep(2)
            return True
        except:
            pass
        return False


# ==================== 主程序 ====================
class UCampusBot:
    """U校园机器人 - 组装所有组件"""

    def __init__(self, config_path: str = 'config.json', skip_check: bool = False):
        self.config = Config.from_json(config_path)
        self.temp_dirs: List[str] = []
        self.driver = None
        self.popup_watcher = None

        # 环境检查
        if not skip_check:
            self._ensure_environment()

        # 创建驱动
        self.driver = self._create_driver()
        self.popup_watcher = PopupWatcher(self.driver)
    def __del__(self):
        self.popup_watcher.stop()
        self.driver.quit()
    def _ensure_environment(self):
        """确保环境就绪"""
        checker = EnvironmentChecker()

        if not checker.check_all():
            # 环境有问题，进入修复流程
            while True:
                choice = checker.show_fix_guide()

                if choice == '1':
                    checker.auto_install_edge()
                    sys.exit(0)

                elif choice == '2':
                    driver_manager = DriverManager()
                    target_dir = os.path.expandvars(r'%LOCALAPPDATA%\U校园AI答题')
                    os.makedirs(target_dir, exist_ok=True)

                    driver_path = checker.auto_download_driver(target_dir)
                    if driver_path:
                        print("✅ 驱动准备完成，请重新运行程序")
                        input("按回车键退出...")
                        sys.exit(0)

                elif choice == '3':
                    # 新增：自动安装 FFmpeg
                    if checker.auto_install_ffmpeg():
                        sys.exit(0)

                elif choice == '4':
                    # 新增：添加 FFmpeg 到 PATH
                    if checker.add_ffmpeg_to_path():
                        sys.exit(0)

                elif choice == '5':
                    # 修改：支持 FFmpeg 路径
                    edge_path, driver_path, ffmpeg_path = checker.manual_specify_path()

                    if edge_path and os.path.exists(edge_path):
                        print(f"✅ 已指定 Edge: {edge_path}")

                    if driver_path:
                        manager = DriverManager()
                        saved_path = manager.save_driver(driver_path)
                        print(f"✅ 驱动已保存: {saved_path}")
                        print("请重新运行程序")
                        input("按回车键退出...")
                        sys.exit(0)

                    # 新增：处理 FFmpeg 路径
                    if ffmpeg_path:
                        bin_dir = os.path.dirname(ffmpeg_path)
                        checker._add_to_system_path(bin_dir)
                        print(f"✅ FFmpeg 已添加到 PATH: {bin_dir}")
                        print("请重新运行程序")
                        input("按回车键退出...")
                        sys.exit(0)

                elif choice == '6':
                    self._show_detailed_help()
                    input("\n按回车键退出...")
                    sys.exit(1)

                elif choice == 'Q':
                    sys.exit(1)

                else:
                    print("无效选项，请重新选择")

    def _show_detailed_help(self):
        """显示详细帮助（添加 FFmpeg 说明）"""
        print("""
    【问题诊断】

    1. Edge 浏览器问题
       原因：Edge 未安装或版本不匹配
       解决：选择 [1] 自动安装，或访问 https://www.microsoft.com/edge

    2. Edge 驱动问题
       原因：msedgedriver.exe 未找到
       解决：选择 [2] 自动下载，或手动放置到程序目录

    3. FFmpeg 问题（语音识别必需）
       原因：未安装 FFmpeg 或未添加到系统 PATH
       解决：
          - 方法A（推荐）：选择 [3] 自动下载安装（约130MB）
          - 方法B：选择 [4] 将已安装的 FFmpeg 添加到 PATH
          - 方法C：手动下载 https://ffmpeg.org/download.html
            解压后将 bin 目录添加到系统环境变量 PATH

    4. 验证 FFmpeg 安装
       打开 CMD 输入: ffmpeg -version
       应显示版本信息，如 "ffmpeg version 6.0"

    【手动安装 FFmpeg 步骤】

    1. 访问 https://ffmpeg.org/download.html
    2. 点击 Windows 图标，选择 "Windows builds from gyan.dev"
    3. 下载 "ffmpeg-release-essentials.zip"
    4. 解压到 C:\ffmpeg
    5. 将 C:\ffmpeg\bin 添加到系统环境变量 PATH
    6. 重启终端，输入 ffmpeg -version 验证
    """)



    def _create_driver(self):
        """创建WebDriver"""
        # self._kill_edge_processes()

        options = webdriver.EdgeOptions()
        options.add_argument('--disable-blink-features=AutomationControlled')
        options.add_experimental_option("excludeSwitches", ["enable-automation"])

        temp_dir = tempfile.mkdtemp(prefix='ucampus_')
        self.temp_dirs.append(temp_dir)
        options.add_argument(f'--user-data-dir={temp_dir}')

        driver = self._try_start_driver(options)

        driver.execute_cdp_cmd("Page.addScriptToEvaluateOnNewDocument", {
            "source": "Object.defineProperty(navigator, 'webdriver', {get: () => false});"
        })

        return driver

    def _try_start_driver(self, options) -> webdriver.Edge:
        """尝试启动驱动"""
        errors = []

        try:
            from selenium.webdriver.edge.service import Service
            from webdriver_manager.microsoft import EdgeChromiumDriverManager

            service = Service(EdgeChromiumDriverManager().install())
            return webdriver.Edge(service=service, options=options)
        except Exception as e:
            errors.append(f"自动管理: {str(e)[:40]}")

        manager = DriverManager()
        user_driver = manager.get_driver_path()
        if user_driver:
            try:
                from selenium.webdriver.edge.service import Service
                service = Service(user_driver)
                return webdriver.Edge(service=service, options=options)
            except Exception as e:
                errors.append(f"用户驱动: {str(e)[:40]}")

        bundled = get_resource_path('msedgedriver.exe')
        if os.path.exists(bundled):
            try:
                from selenium.webdriver.edge.service import Service
                service = Service(bundled)
                return webdriver.Edge(service=service, options=options)
            except Exception as e:
                errors.append(f"自带驱动: {str(e)[:40]}")

        try:
            return webdriver.Edge(options=options)
        except Exception as e:
            errors.append(f"系统PATH: {str(e)[:40]}")

        print("\n❌ 浏览器启动失败:")
        for err in errors:
            print(f"   - {err}")
        raise Exception("无法启动 Edge 浏览器")

    def _kill_edge_processes(self):
        """结束 Edge 进程"""
        try:
            subprocess.run(['taskkill', '/F', '/IM', 'msedge.exe'],
                           capture_output=True, check=False)
            time.sleep(0.5)
        except:
            pass


    def start(self):
        """启动"""
        if not self._login():
            return False
        # 启动弹窗监控
        self.popup_watcher=threading.Thread(target=self.popup_watcher.run, daemon=True)
        self.popup_watcher.start()
        # 开始学习
        learner = CourseLearner(self.driver, self.config)
        return learner.learn()

    def _login(self) -> bool:
        """登录"""
        try:
            self.driver.get(self.config.url)
            time.sleep(3)
            # 填写登录信息
            username = WebDriverWait(self.driver, 20).until(
                EC.presence_of_element_located((By.XPATH, '//*[@id="username"]')))
            password = self.driver.find_element(By.XPATH, '//*[@id="password"]')
            agreement_check = WebDriverWait(self.driver, 20).until(
                EC.presence_of_element_located((By.XPATH, '//*[@id="agreement"]')))
            username.send_keys(self.config.username)
            password.send_keys(self.config.password)
            if not agreement_check.is_selected():
                agreement_check.click()
            # 点击登录
            login_btn = self.driver.find_element(By.XPATH,
                                                 '//*[@id="rc-tabs-0-panel-1"]/form/div[4]/div/div/div/div/button')
            login_btn.click()
            winsound.MessageBeep()
            input("如有验证码，请手动输入验证码后在此处回车；如没有则直接按回车")
            self.anti_anti_cheat()
            time.sleep(3)
            try:
                zhidaole_button = WebDriverWait(self.driver, 5).until(EC.presence_of_element_located((By.XPATH,
                                                                                                      '/html/body/div[3]/div/div[2]/div/div[2]/div/div/div/div[4]/button')))
                zhidaole_button.click()
            except Exception:
                print(" 没有知道了按钮，已跳过")

            try:
                anti_cheat_announce_button = WebDriverWait(self.driver, 5).until(
                    EC.presence_of_element_located((By.XPATH,
                                                    '/html/body/div[4]/div/div/div/div[2]/div/div/div[4]/div[5]')))
                anti_cheat_announce_button.click()
            except Exception:
                print(' 没有找到防作弊弹窗，已跳过')
            print("✅ 登录成功")
            return True

        except Exception as e:
            error_msg = str(e)
            print(f"❌ 登录失败: {error_msg[:50]}")
            logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件
            return False

    def anti_anti_cheat(self):
        """注入token绕过防作弊检测"""
        self.driver.execute_script('window.localStorage.setItem("__token", `{}`);'.format(self.config.token_full))
        self.driver.refresh()


class PopupWatcher:
    """弹窗监控器"""

    def __init__(self, driver):
        self.driver = driver
        self.running = False

    def run(self):
        """运行监控"""
        self.running = True
        while self.running:
            try:
                self._click_known_buttons()
                time.sleep(0.5)
            except Exception as e:
                error_msg = str(e)
                print(f"操作失败: {error_msg[:50]}")  # 控制台只显示简短信息
                logger.error(f"详细错误: {error_msg}", exc_info=True)  # 详细堆栈保存到文件

    def _click_known_buttons(self):
        """点击已知按钮"""
        js = """
        function findBtn(w) {
            const selectors = [
                '.know-box .iKnow',
                '.ant-modal-confirm-btns .ant-btn-primary',
                '.system-info-cloud-ok-button'
            ];
            for (let sel of selectors) {
                const b = w.document.querySelector(sel);
                if (b) return b;
            }
            return null;
        }
        function clickBtn(btn) {
            ['mouseover','mousedown','mouseup','click'].forEach(ev => {
                btn.dispatchEvent(new MouseEvent(ev, {bubbles:true}));
            });
            btn.click();
        }
        let btn = findBtn(window);
        if (btn) { clickBtn(btn); return true; }
        for (let i=0; i<window.frames.length; i++) {
            try {
                btn = findBtn(window.frames[i]);
                if (btn) { clickBtn(btn); return true; }
            } catch(e) {}
        }
        return false;
        """
        self.driver.execute_script(js)

    def stop(self):
        self.running = False


if __name__ == '__main__':
    print('*' * 25 + "U校园AI答题" + '*' * 25)
    print("\033[4;32m作者B站ID：看了吴钩系钓舟\033[m")

    skip_check = '--skip-check' in sys.argv

    if not skip_check:
        print("\n💡 提示：")
        print("   - 首次运行需要检查环境")
        print("   - 语音识别需要 FFmpeg（约130MB，可自动安装）")
        print("   - 如检查通过但无法启动，使用 --skip-check 跳过")
        print("   命令: U校园AI答题.exe --skip-check\n")

    input('按任意键启动程序...')

    logger, LOG_FILE = setup_logging()
    print(f"📄 详细日志保存至: {LOG_FILE}")

    try:
        bot = UCampusBot('config.json', skip_check=skip_check)
        bot.start()
    except Exception as e:
        error_msg = str(e)
        print(f"\n❌ 程序运行失败: {error_msg[:100]}")
        logger.error(f"程序异常: {error_msg}", exc_info=True)

        print("\n💡 建议：")
        print("   1. 重新运行程序，选择环境修复选项")
        print("   2. 以管理员身份运行程序")
        print("   3. 关闭杀毒软件后重试")
        print("   4. 查看日志文件获取详细信息")

        input("\n按回车键退出...")