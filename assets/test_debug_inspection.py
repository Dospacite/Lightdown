"""Run with python3 -B -m unittest discover -s assets -p test_debug_inspection.py."""
import itertools
import pathlib
import sys
import unittest

namespace = {}
exec(pathlib.Path(__file__).with_name('debugger.py').read_text().split('initial = json.loads')[0], namespace)
namespace['project'] = str(pathlib.Path(__file__).parent)
children, describe, resolve = namespace['children'], namespace['describe'], namespace['resolve']


class Trap:
    """Every hook that could run code fails the test when called."""
    __slots__ = ('item', '__dict__')
    def __repr__(self): raise AssertionError('repr')
    def __str__(self): raise AssertionError('str')
    def __getattr__(self, name): raise AssertionError('getattr')
    def __getattribute__(self, name): raise AssertionError('getattribute')
    def __len__(self): raise AssertionError('len')
    def __iter__(self): raise AssertionError('iter')
    @property
    def trap(self): raise AssertionError('property')
    def method(self): raise AssertionError('method')


def trap():
    obj = object.__new__(Trap)
    object.__setattr__(obj, 'item', 42)
    object.__setattr__(obj, 'other', [1, 2])
    return obj


class TrapList(list):
    def __repr__(self): raise AssertionError('repr')
    def __iter__(self): raise AssertionError('iter')
    def __len__(self): raise AssertionError('len')


class Inspection(unittest.TestCase):
    def test_storage_does_not_call_user_code(self):
        obj = trap()
        self.assertEqual(dict(children(obj)), dict(item=42, other=[1, 2]))
        self.assertEqual(describe(obj), '<Trap object>')
        self.assertTrue(namespace['has_children'](obj))

    def test_containers_describe_and_page(self):
        self.assertEqual(list(children({'a': 3})), [("'a'", 3)])
        self.assertEqual(len(list(itertools.islice(children(list(range(450))), 200, 400))), 200)
        self.assertEqual(list(children((1, 2))), [('0', 1), ('1', 2)])
        self.assertEqual(len(list(children({1, 2}))), 2)
        self.assertEqual(describe([1, 'a', (2,)]), "[1, 'a', (2,)]")
        self.assertEqual(describe({'k': [trap()]}), "{'k': [<Trap object>]}")
        self.assertEqual(describe(list(range(20))), '[0, 1, 2, 3, 4, 5, 6, 7, …]')
        self.assertEqual(describe(set()), 'set()')
        self.assertEqual(describe(10 ** 2000), '<int of 6644 bits>')
        self.assertFalse(namespace['has_children'](object()))

    def test_container_subclasses_use_base_storage(self):
        items = TrapList([1, 2])
        self.assertEqual(describe(items), '<TrapList of 2>')
        self.assertEqual(list(children(items)), [('0', 1), ('1', 2)])

    def test_hover_resolves_names_and_stored_attributes_only(self):
        class Frame:
            f_locals = {'self': trap()}
            f_globals = {'__builtins__': __builtins__, 'sys': sys}
        self.assertEqual(resolve('self.item', Frame), 42)
        self.assertEqual(resolve('self.other', Frame), [1, 2])
        self.assertIs(resolve('len', Frame), len)
        self.assertIs(resolve('sys.path', Frame), sys.path)
        for expression in ('self.trap', 'self.missing', 'self.item()', 'a[0]', 'nothing'):
            with self.assertRaises(LookupError, msg=expression):
                resolve(expression, Frame)
        self.assertEqual(describe(resolve('self.method', Frame)), '<function method>')

    def test_exception_messages_skip_user_str(self):
        text = namespace['exception_text']
        self.assertEqual(text(ValueError('bad value')), 'bad value')
        self.assertEqual(text(KeyError('k')), "'k'")
        class Custom(Exception):
            def __str__(self): raise AssertionError('str')
        self.assertEqual(text(Custom('x', 2)), "'x', 2")
        self.assertEqual(text(ValueError(trap())), '<Trap object>')


if __name__ == '__main__':
    unittest.main()
