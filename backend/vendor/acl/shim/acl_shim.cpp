// C ABI over ACL's header-only decompressor, used by the `acl` cargo feature (rig animation).
#include <acl/decompression/decompress.h>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>

namespace
{
	// ACL reads compressed buffers in place and requires 16-byte alignment.
	struct aligned_copy
	{
		void* ptr = nullptr;
		aligned_copy(const uint8_t* data, size_t len)
		{
			if (len == 0 || len > SIZE_MAX - 15)
				return;
			ptr = std::aligned_alloc(16, (len + 15) / 16 * 16);
			if (ptr != nullptr)
				std::memcpy(ptr, data, len);
		}
		~aligned_copy() { std::free(ptr); }
		const acl::compressed_tracks* tracks() const { return static_cast<const acl::compressed_tracks*>(ptr); }
	};

	// Writes rotation xyzw, translation xyz and scale xyz: 10 floats per track.
	struct transform_writer final : public acl::track_writer
	{
		float* out;
		explicit transform_writer(float* out_) : out(out_) {}
		void RTM_SIMD_CALL write_rotation(uint32_t i, rtm::quatf_arg0 q) { rtm::quat_store(q, out + i * 10); }
		void RTM_SIMD_CALL write_translation(uint32_t i, rtm::vector4f_arg0 t) { rtm::vector_store3(t, out + i * 10 + 4); }
		void RTM_SIMD_CALL write_scale(uint32_t i, rtm::vector4f_arg0 s) { rtm::vector_store3(s, out + i * 10 + 7); }
	};

	struct float_writer final : public acl::track_writer
	{
		float* out;
		explicit float_writer(float* out_) : out(out_) {}
		void RTM_SIMD_CALL write_float1(uint32_t i, rtm::scalarf_arg0 v) { out[i] = rtm::scalar_cast(v); }
	};

	// Header checks before trusting anything ACL reads from the buffer: the raw and tracks
	// headers fit, the declared size lies between them and `len` (ACL hashes `size - 8` bytes),
	// the hash validates and multi-sample tracks have a finite positive sample rate.
	bool valid_buffer(const uint8_t* data, size_t len, const aligned_copy& copy)
	{
		constexpr size_t min_size = sizeof(acl::acl_impl::raw_buffer_header) + sizeof(acl::acl_impl::tracks_header);
		if (data == nullptr || len < min_size || copy.ptr == nullptr)
			return false;
		const acl::compressed_tracks* tracks = copy.tracks();
		const size_t size = tracks->get_size();
		if (size < min_size || size > len)
			return false;
		if (tracks->is_valid(true).any())
			return false;
		const float rate = tracks->get_sample_rate();
		if (tracks->get_num_samples_per_track() > 1 && !(std::isfinite(rate) && rate > 0.0F))
			return false;
		return true;
	}

	// tracks * samples * per_track floats, or false on overflow or when above `out_len`.
	bool output_fits(uint32_t tracks, uint32_t samples, size_t per_track, size_t out_len)
	{
		size_t n = 0;
		if (__builtin_mul_overflow(size_t(tracks), size_t(samples), &n) || __builtin_mul_overflow(n, per_track, &n))
			return false;
		return n <= out_len;
	}
}

extern "C" {

// Header of a compressed buffer. Returns 0 when valid, a negative error otherwise.
int fluorite_acl_info(const uint8_t* data, size_t len, uint32_t* num_tracks, uint32_t* num_samples, float* sample_rate, uint8_t* track_type)
{
	if (data == nullptr)
		return -1;
	aligned_copy copy(data, len);
	if (!valid_buffer(data, len, copy))
		return -3;
	const acl::compressed_tracks* tracks = copy.tracks();
	*num_tracks = tracks->get_num_tracks();
	*num_samples = tracks->get_num_samples_per_track();
	*sample_rate = tracks->get_sample_rate();
	*track_type = static_cast<uint8_t>(tracks->get_track_type());
	return 0;
}

// Decompress every sample of a transform (qvvf) buffer into
// out[sample * num_tracks * 10 + track * 10 + k]. `out_len` is in floats.
int fluorite_acl_transforms(const uint8_t* data, size_t len, float* out, size_t out_len)
{
	aligned_copy copy(data, len);
	if (!valid_buffer(data, len, copy) || copy.tracks()->get_track_type() != acl::track_type8::qvvf)
		return -1;
	const acl::compressed_tracks* tracks = copy.tracks();
	const uint32_t n = tracks->get_num_tracks();
	const uint32_t samples = tracks->get_num_samples_per_track();
	if (!output_fits(n, samples, 10, out_len))
		return -2;
	acl::decompression_context<acl::debug_transform_decompression_settings> context;
	if (!context.initialize(*tracks))
		return -3;
	const float rate = tracks->get_sample_rate();
	for (uint32_t s = 0; s < samples; ++s)
	{
		transform_writer writer(out + size_t(s) * n * 10);
		context.seek(float(s) / rate, acl::sample_rounding_policy::nearest);
		context.decompress_tracks(writer);
	}
	return 0;
}

// Decompress every sample of a scalar float1 buffer into out[sample * num_tracks + track].
int fluorite_acl_floats(const uint8_t* data, size_t len, float* out, size_t out_len)
{
	aligned_copy copy(data, len);
	if (!valid_buffer(data, len, copy) || copy.tracks()->get_track_type() != acl::track_type8::float1f)
		return -1;
	const acl::compressed_tracks* tracks = copy.tracks();
	const uint32_t n = tracks->get_num_tracks();
	const uint32_t samples = tracks->get_num_samples_per_track();
	if (!output_fits(n, samples, 1, out_len))
		return -2;
	acl::decompression_context<acl::debug_scalar_decompression_settings> context;
	if (!context.initialize(*tracks))
		return -3;
	const float rate = tracks->get_sample_rate();
	for (uint32_t s = 0; s < samples; ++s)
	{
		float_writer writer(out + size_t(s) * n);
		context.seek(float(s) / rate, acl::sample_rounding_policy::nearest);
		context.decompress_tracks(writer);
	}
	return 0;
}

}
