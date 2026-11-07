//! Python bindings (module `npids._npids`). The public Python API is in `python/npids`.

use crate::{Error, Options};
use numpy::{PyArray1, PyReadonlyArray1};
use pyo3::exceptions::{PyIndexError, PyOSError, PyRuntimeError, PyTypeError, PyUnicodeDecodeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyString};
use std::path::PathBuf;

fn to_py(e: Error) -> PyErr {
    match e {
        Error::Io(e) => PyOSError::new_err(e.to_string()),
        Error::OutOfRange(_) => PyIndexError::new_err(e.to_string()),
        Error::NoInverse => PyRuntimeError::new_err(e.to_string()),
        Error::Corrupt(_) | Error::Unsupported(_) | Error::Invalid(_) => PyValueError::new_err(e.to_string()),
    }
}

/// Borrow the bytes of a str (UTF-8) or bytes object.
fn id_bytes<'a>(obj: &'a Bound<'_, PyAny>) -> PyResult<std::borrow::Cow<'a, [u8]>> {
    if let Ok(b) = obj.cast::<PyBytes>() {
        return Ok(std::borrow::Cow::Borrowed(b.as_bytes()));
    }
    if let Ok(s) = obj.cast::<PyString>() {
        return Ok(match s.to_cow()? {
            std::borrow::Cow::Borrowed(s) => std::borrow::Cow::Borrowed(s.as_bytes()),
            std::borrow::Cow::Owned(s) => std::borrow::Cow::Owned(s.into_bytes()),
        });
    }
    Err(PyTypeError::new_err(format!("IDs must be str or bytes, not {}", obj.get_type().name()?)))
}

fn id_to_py<'py>(py: Python<'py>, id: &[u8], as_bytes: bool) -> PyResult<Bound<'py, PyAny>> {
    if as_bytes {
        return Ok(PyBytes::new(py, id).into_any());
    }
    match std::str::from_utf8(id) {
        Ok(s) => Ok(PyString::new(py, s).into_any()),
        Err(e) => Err(PyErr::from_value(PyUnicodeDecodeError::new_utf8(py, id, e)?.into_any())),
    }
}

#[pyclass(frozen, module = "npids._npids")]
struct Reader {
    inner: crate::Reader,
}

impl Reader {
    fn norm(&self, idx: i64) -> PyResult<u64> {
        let n = self.inner.len() as i64;
        let i = if idx < 0 { idx + n } else { idx };
        if i < 0 || i >= n {
            return Err(PyIndexError::new_err(format!("index {idx} is out of range for {n} IDs")));
        }
        Ok(i as u64)
    }
}

#[pymethods]
impl Reader {
    #[new]
    fn new(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        let inner = py.detach(|| crate::Reader::open(&path)).map_err(to_py)?;
        Ok(Reader { inner })
    }

    fn __len__(&self) -> usize {
        self.inner.len() as usize
    }

    #[pyo3(signature = (idx, as_bytes=false))]
    fn get<'py>(&self, py: Python<'py>, idx: i64, as_bytes: bool) -> PyResult<Bound<'py, PyAny>> {
        let i = self.norm(idx)?;
        let mut buf = Vec::new();
        self.inner.get(i, &mut buf).map_err(to_py)?;
        id_to_py(py, &buf, as_bytes)
    }

    /// Look up many indices (negative ones count from the end); returns a list.
    #[pyo3(signature = (idxs, as_bytes=false))]
    fn get_many<'py>(&self, py: Python<'py>, idxs: PyReadonlyArray1<'py, i64>, as_bytes: bool) -> PyResult<Bound<'py, PyList>> {
        let idxs: Vec<u64> = idxs.as_array().iter().map(|&i| self.norm(i)).collect::<PyResult<_>>()?;
        let (blob, ends) = py
            .detach(|| {
                let mut blob = Vec::with_capacity(idxs.len() * 16);
                let mut ends = Vec::with_capacity(idxs.len());
                self.inner.get_many(&idxs, &mut blob, &mut ends)?;
                Ok::<_, Error>((blob, ends))
            })
            .map_err(to_py)?;
        let mut items = Vec::with_capacity(ends.len());
        let mut s = 0;
        for e in ends {
            items.push(id_to_py(py, &blob[s..e], as_bytes)?);
            s = e;
        }
        PyList::new(py, items)
    }

    /// Look up many indices without creating a Python object per ID. Returns `(offsets, data)`:
    /// an int64 array of length n+1 and a bytes object, so ID `i` is `data[offsets[i]:offsets[i+1]]`
    /// (the layout of an Arrow large string array).
    fn get_many_raw<'py>(&self, py: Python<'py>, idxs: PyReadonlyArray1<'py, i64>) -> PyResult<(Bound<'py, PyArray1<i64>>, Bound<'py, PyBytes>)> {
        let idxs: Vec<u64> = idxs.as_array().iter().map(|&i| self.norm(i)).collect::<PyResult<_>>()?;
        let (blob, offsets) = py
            .detach(|| {
                let mut blob = Vec::with_capacity(idxs.len() * 16);
                let mut ends = Vec::with_capacity(idxs.len());
                self.inner.get_many(&idxs, &mut blob, &mut ends)?;
                let mut offsets = Vec::with_capacity(idxs.len() + 1);
                offsets.push(0i64);
                offsets.extend(ends.iter().map(|&e| e as i64));
                Ok::<_, Error>((blob, offsets))
            })
            .map_err(to_py)?;
        Ok((PyArray1::from_vec(py, offsets), PyBytes::new(py, &blob)))
    }

    /// The first index of an ID (str or bytes), or None.
    fn find(&self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<Option<u64>> {
        let id = id_bytes(id)?;
        py.detach(|| self.inner.find(&id)).map_err(to_py)
    }

    /// First indices of many IDs, as an int64 array with -1 for IDs that are not found.
    fn find_many<'py>(&self, py: Python<'py>, ids: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyArray1<i64>>> {
        let mut blob = Vec::new();
        let mut ends = Vec::new();
        for id in ids.try_iter()? {
            let id = id?;
            blob.extend_from_slice(&id_bytes(&id)?);
            ends.push(blob.len());
        }
        let out = py
            .detach(|| {
                let mut out = Vec::with_capacity(ends.len());
                let mut s = 0;
                for &e in &ends {
                    out.push(self.inner.find(&blob[s..e])?.map_or(-1, |i| i as i64));
                    s = e;
                }
                Ok::<_, Error>(out)
            })
            .map_err(to_py)?;
        Ok(PyArray1::from_vec(py, out))
    }

    fn has_full_inverse(&self) -> bool {
        self.inner.inverse_coverage() == self.inner.len()
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn meta<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.meta())
    }

    #[pyo3(signature = (deep=false))]
    fn verify(&self, py: Python<'_>, deep: bool) -> PyResult<()> {
        py.detach(|| self.inner.verify(deep)).map_err(to_py)
    }
}

#[pyclass(module = "npids._npids")]
struct Writer {
    inner: Option<crate::Writer>,
}

impl Writer {
    fn w(&mut self) -> PyResult<&mut crate::Writer> {
        self.inner.as_mut().ok_or_else(|| PyValueError::new_err("this builder is closed"))
    }
}

#[pymethods]
impl Writer {
    #[new]
    #[pyo3(signature = (path, chunk_bits=16, build_inv=true, meta=None))]
    fn new(py: Python<'_>, path: PathBuf, chunk_bits: u8, build_inv: bool, meta: Option<Vec<u8>>) -> PyResult<Self> {
        let opts = Options { chunk_bits, build_inv, meta };
        let inner = py.detach(|| crate::Writer::open(&path, opts)).map_err(to_py)?;
        Ok(Writer { inner: Some(inner) })
    }

    fn add(&mut self, id: &Bound<'_, PyAny>) -> PyResult<()> {
        let id = id_bytes(id)?;
        self.w()?.add(&id).map_err(to_py)
    }

    fn add_many(&mut self, ids: &Bound<'_, PyAny>) -> PyResult<()> {
        let w = self.w()?;
        for id in ids.try_iter()? {
            let id = id?;
            w.add(&id_bytes(&id)?).map_err(to_py)?;
        }
        Ok(())
    }

    fn __len__(&self) -> usize {
        self.inner.as_ref().map_or(0, |w| w.len() as usize)
    }

    /// Write everything and commit.
    fn finish(&mut self, py: Python<'_>) -> PyResult<()> {
        let w = self.inner.take().ok_or_else(|| PyValueError::new_err("this builder is closed"))?;
        py.detach(|| w.finish()).map_err(to_py)
    }

    /// Discard everything added since the builder was opened.
    fn abort(&mut self) {
        self.inner = None;
    }
}

#[pyfunction]
#[pyo3(signature = (path, chunk_bits=None))]
fn compact(py: Python<'_>, path: PathBuf, chunk_bits: Option<u8>) -> PyResult<()> {
    py.detach(|| crate::compact(&path, chunk_bits)).map_err(to_py)
}

#[pymodule]
fn _npids(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Reader>()?;
    m.add_class::<Writer>()?;
    m.add_function(wrap_pyfunction!(compact, m)?)?;
    Ok(())
}
